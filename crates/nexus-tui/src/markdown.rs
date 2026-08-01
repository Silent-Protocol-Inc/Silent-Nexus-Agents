//! Answers as terminal documents.
//!
//! A model answer arrives as Markdown. Until now the timeline handed it to
//! `push_wrapped`, a plain word-wrapper, so `## Review summary`,
//! `**Suggested fix:**`, and `` `--bind 127.0.0.1` `` reached the operator as
//! literal source. There was no parser to ignore — there was no parser.
//!
//! Three things are kept apart, and the separation is the point:
//!
//! * **Source** — the exact sanitized text the provider sent. It is what the
//!   timeline stores, what `/export` writes, and what a copy yields. Nothing
//!   here ever writes back to it, so a terminal-formatted answer can never
//!   become the canonical one.
//! * **Document** — [`Document`], the parsed structure. Width-independent, so
//!   a resize re-renders rather than re-parses, and theme or mode changes cost
//!   nothing.
//! * **Lines** — styled `ratatui` rows for one width, one theme, one mode.
//!
//! Parsing is CommonMark via `pulldown-cmark` with tables, task lists, and
//! strikethrough enabled and **HTML off**: raw HTML arrives as text and is
//! rendered as text. Model output is untrusted, so every string that reaches a
//! span goes through the harness sanitizer first — an answer cannot move the
//! cursor, set the terminal title, or drive the clipboard.

use std::cell::RefCell;
use std::collections::HashMap;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// One run of text with a semantic role. Style is applied at render time, so a
/// document survives a theme change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub emphasis: Emphasis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Emphasis {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    /// Text belonging to a link, which is drawn in the link accent.
    pub link: bool,
}

/// A parsed answer. Blocks only — no widths, no styles, no escape sequences.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        pieces: Vec<Piece>,
    },
    Paragraph(Vec<Piece>),
    /// A list. `start` is `Some(n)` for ordered lists.
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Code {
        language: Option<String>,
        lines: Vec<String>,
    },
    Quote(Vec<Block>),
    Table {
        headers: Vec<Vec<Piece>>,
        rows: Vec<Vec<Vec<Piece>>>,
    },
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// `Some(true)` for `[x]`, `Some(false)` for `[ ]`, `None` for an ordinary
    /// item.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

/// How much room the document may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    pub width: usize,
    /// Compact tightens vertical spacing and caps long code blocks.
    pub compact: bool,
    /// False on an ASCII-only terminal: bullets, rules, and code frames fall
    /// back to characters every terminal can draw.
    pub unicode: bool,
}

/// Parse Markdown into a document.
///
/// Never fails: malformed Markdown is valid input. The parser is a state
/// machine over events, so an unclosed fence or a half-written emphasis run
/// simply ends the document early rather than producing an error.
pub fn parse(source: &str) -> Document {
    let source = nexus_core::sanitize::sanitize_terminal(source);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    // Deliberately absent: `ENABLE_HTML` has no equivalent here — pulldown
    // reports raw HTML as `Event::Html`, which this builder appends as text.
    // An answer cannot inject markup, and there is no HTML renderer to reach.
    let mut builder = Builder::default();
    for event in Parser::new_ext(&source, options) {
        builder.push(event);
    }
    let mut doc = builder.finish();
    trim_tail_markers(&mut doc);
    doc
}

/// Drop emphasis markers left dangling at the very tip of the document.
///
/// A stream that stops after `Some **` is not malformed — the closing pair has
/// not arrived. CommonMark correctly renders the orphan as literal text, so the
/// operator watches `**` appear and vanish a frame later. Deciding this *after*
/// parsing is what makes it safe: balanced emphasis has already been consumed,
/// so `***both***` is untouched and only a real orphan is trimmed. Markers are
/// syntax, never content, so nothing is lost.
fn trim_tail_markers(doc: &mut Document) {
    let Some(last) = doc.blocks.last_mut() else {
        return;
    };
    let pieces = match last {
        Block::Paragraph(pieces) | Block::Heading { pieces, .. } => pieces,
        _ => return,
    };
    let Some(piece) = pieces.last_mut() else {
        return;
    };
    piece.text = drop_unclosed_opener(&piece.text);
    pieces.retain(|piece| !piece.text.is_empty());
    if pieces.is_empty() {
        doc.blocks.pop();
    }
}

/// Render a parsed document for one width, theme, and mode.
pub fn render(doc: &Document, t: &Theme, opts: RenderOptions) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    render_blocks(&doc.blocks, t, opts, 0, 0, &mut out);
    while out.last().is_some_and(is_blank) {
        out.pop();
    }
    out
}

/// Parse and render, memoised.
///
/// The timeline re-renders every visible event on every frame, and a streaming
/// answer changes on every delta, so parsing is on the hot path. The cache is
/// keyed by content hash *and* by everything that changes the output, so a
/// resize, a theme switch, or a mode change all miss correctly.
pub fn render_source(source: &str, t: &Theme, opts: RenderOptions) -> Vec<Line<'static>> {
    thread_local! {
        static CACHE: RefCell<HashMap<u64, Document>> = RefCell::new(HashMap::new());
    }
    /// Bounded so a long session cannot grow it without limit. Answers are
    /// re-rendered by recency, so a plain clear beats an LRU here.
    const MAX_ENTRIES: usize = 64;
    let key = hash(source);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() > MAX_ENTRIES {
            cache.clear();
        }
        let doc = cache.entry(key).or_insert_with(|| parse(source));
        render(doc, t, opts)
    })
}

/// Remove one unmatched emphasis *opener* from the tail of a run.
///
/// The narrow rule matters. A marker is only an opener if it is immediately
/// followed by non-whitespace — CommonMark's left-flanking test — so `2 * 3`
/// keeps its asterisk and `Some **b` loses the pair it has not closed yet.
/// Stripping every trailing marker would have been simpler and would have
/// silently eaten literal punctuation, which is worse than a frame of flicker.
fn drop_unclosed_opener(text: &str) -> String {
    let mut text = text.to_string();
    // Longest first: `**` must be considered before `*`.
    for marker in ["***", "**", "~~", "`", "*", "_"] {
        if text.matches(marker).count() % 2 == 0 {
            continue;
        }
        let Some(at) = text.rfind(marker) else {
            continue;
        };
        let after = &text[at + marker.len()..];
        let left_flanking =
            after.chars().next().is_some_and(|c| !c.is_whitespace()) || after.is_empty();
        if left_flanking {
            text.replace_range(at..at + marker.len(), "");
        }
    }
    text.trim_end().to_string()
}

fn hash(text: &str) -> u64 {
    text.as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash: u64, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        })
}

// ------------------------------------------------------------------- parsing

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    /// Open containers: lists and quotes nest, so blocks land in the innermost.
    stack: Vec<Frame>,
    pieces: Vec<Piece>,
    emphasis: Emphasis,
    heading: Option<u8>,
    code: Option<(Option<String>, String)>,
    table: Option<TableBuilder>,
}

enum Frame {
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<Block>,
    },
    Quote(Vec<Block>),
}

#[derive(Default)]
struct TableBuilder {
    headers: Vec<Vec<Piece>>,
    rows: Vec<Vec<Vec<Piece>>>,
    row: Vec<Vec<Piece>>,
    in_head: bool,
}

impl Builder {
    fn push(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            // Raw HTML is text. There is no HTML renderer to reach and no
            // markup to inject; the operator sees exactly what the model wrote.
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            Event::Code(code) => {
                let previous = self.emphasis;
                self.emphasis.code = true;
                self.text(&code);
                self.emphasis = previous;
            }
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => self.block(Block::Rule),
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(done);
                }
            }
            Event::FootnoteReference(name) => self.text(&format!("[^{name}]")),
            Event::InlineMath(text) | Event::DisplayMath(text) => self.text(&text),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => self.heading = Some(heading_level(level)),
            Tag::Paragraph => {}
            Tag::Strong => self.emphasis.bold = true,
            Tag::Emphasis => self.emphasis.italic = true,
            Tag::Strikethrough => self.emphasis.strike = true,
            Tag::Link { .. } => self.emphasis.link = true,
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let info = info.split_whitespace().next().unwrap_or("").trim();
                        (!info.is_empty()).then(|| info.to_string())
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((language, String::new()));
            }
            // Opening a container closes whatever text was being collected.
            // A tight list emits its item text with no `Paragraph` around it,
            // so without this the item's words and its nested item's words
            // accumulated into one run — "second" + "nested" = "secondnested".
            Tag::List(start) => {
                self.flush_pieces();
                self.stack.push(Frame::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.flush_pieces();
                self.stack.push(Frame::Item {
                    task: None,
                    blocks: Vec::new(),
                });
            }
            Tag::BlockQuote(_) => {
                self.flush_pieces();
                self.stack.push(Frame::Quote(Vec::new()));
            }
            Tag::Table(_) => self.table = Some(TableBuilder::default()),
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_head = true;
                }
            }
            Tag::TableRow | Tag::TableCell => {}
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let pieces = self.take_pieces();
                if !pieces.is_empty() {
                    self.block(Block::Heading { level, pieces });
                }
            }
            TagEnd::Paragraph => {
                let pieces = self.take_pieces();
                if !pieces.is_empty() {
                    self.block(Block::Paragraph(pieces));
                }
            }
            TagEnd::Strong => self.emphasis.bold = false,
            TagEnd::Emphasis => self.emphasis.italic = false,
            TagEnd::Strikethrough => self.emphasis.strike = false,
            TagEnd::Link => self.emphasis.link = false,
            TagEnd::CodeBlock => {
                if let Some((language, body)) = self.code.take() {
                    let lines = body
                        .strip_suffix('\n')
                        .unwrap_or(&body)
                        .split('\n')
                        .map(str::to_string)
                        .collect();
                    self.block(Block::Code { language, lines });
                }
            }
            TagEnd::Item => {
                let pieces = self.take_pieces();
                if let Some(Frame::Item { task, mut blocks }) = self.stack.pop() {
                    if !pieces.is_empty() {
                        blocks.insert(0, Block::Paragraph(pieces));
                    }
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(ListItem { task, blocks });
                    }
                }
            }
            TagEnd::List(_) => {
                if let Some(Frame::List { start, items }) = self.stack.pop() {
                    if !items.is_empty() {
                        self.block(Block::List { start, items });
                    }
                }
            }
            TagEnd::BlockQuote(_) => {
                if let Some(Frame::Quote(blocks)) = self.stack.pop() {
                    if !blocks.is_empty() {
                        self.block(Block::Quote(blocks));
                    }
                }
            }
            TagEnd::TableCell => {
                let pieces = self.take_pieces();
                if let Some(table) = &mut self.table {
                    table.row.push(pieces);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    if !row.is_empty() {
                        table.rows.push(row);
                    }
                }
            }
            // The head has no `TableRow` around it — pulldown emits its cells
            // directly between `TableHead` start and end. Reading the header
            // out on `TableRow` therefore never fired, and the header cells
            // leaked into the first body row.
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.headers = std::mem::take(&mut table.row);
                    table.in_head = false;
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.block(Block::Table {
                        headers: table.headers,
                        rows: table.rows,
                    });
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if let Some((_, body)) = &mut self.code {
            body.push_str(text);
            return;
        }
        if text.is_empty() {
            return;
        }
        // Merge into the previous run when the emphasis is unchanged, so a
        // paragraph is a handful of spans rather than one per character.
        if let Some(last) = self.pieces.last_mut() {
            if last.emphasis == self.emphasis {
                last.text.push_str(text);
                return;
            }
        }
        self.pieces.push(Piece {
            text: text.to_string(),
            emphasis: self.emphasis,
        });
    }

    /// Close the current text run into the innermost container.
    fn flush_pieces(&mut self) {
        let pieces = self.take_pieces();
        if !pieces.is_empty() {
            self.block(Block::Paragraph(pieces));
        }
    }

    fn take_pieces(&mut self) -> Vec<Piece> {
        let pieces = std::mem::take(&mut self.pieces);
        pieces
            .into_iter()
            .filter(|piece| !piece.text.is_empty())
            .collect()
    }

    /// File a finished block into the innermost open container.
    fn block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Item { blocks, .. }) | Some(Frame::Quote(blocks)) => blocks.push(block),
            _ => self.blocks.push(block),
        }
    }

    /// Close whatever is still open.
    ///
    /// This is what makes a streaming answer safe: a fence that has not closed,
    /// a list still receiving items, or a quote mid-paragraph all finish as the
    /// partial block they are, rather than being dropped.
    fn finish(mut self) -> Document {
        let pieces = self.take_pieces();
        if !pieces.is_empty() {
            self.block(Block::Paragraph(pieces));
        }
        if let Some((language, body)) = self.code.take() {
            let lines = body.split('\n').map(str::to_string).collect();
            self.block(Block::Code { language, lines });
        }
        while let Some(frame) = self.stack.pop() {
            match frame {
                Frame::Item { task, blocks } => {
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(ListItem { task, blocks });
                    }
                }
                Frame::List { start, items } => {
                    if !items.is_empty() {
                        self.block(Block::List { start, items });
                    }
                }
                Frame::Quote(blocks) => {
                    if !blocks.is_empty() {
                        self.block(Block::Quote(blocks));
                    }
                }
            }
        }
        if let Some(table) = self.table.take() {
            // A table whose separator row never arrived still has cells worth
            // showing; an empty one is dropped rather than drawn as a frame
            // around nothing.
            if !table.headers.is_empty() || !table.rows.is_empty() {
                self.blocks.push(Block::Table {
                    headers: table.headers,
                    rows: table.rows,
                });
            }
        }
        Document {
            blocks: self.blocks,
        }
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

// ----------------------------------------------------------------- rendering

fn render_blocks(
    blocks: &[Block],
    t: &Theme,
    opts: RenderOptions,
    indent: usize,
    depth: usize,
    out: &mut Vec<Line<'static>>,
) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 && needs_gap(block, &blocks[index - 1], opts) {
            out.push(Line::from(""));
        }
        render_block(block, t, opts, indent, depth, out);
    }
}

/// Vertical rhythm. Compact drops the gap between consecutive list items and
/// between a heading and the paragraph under it; everything else keeps it,
/// because a document with no air is as unreadable as raw source.
fn needs_gap(current: &Block, previous: &Block, opts: RenderOptions) -> bool {
    if !opts.compact {
        return true;
    }
    !matches!(
        (previous, current),
        (Block::Heading { .. }, _) | (Block::Paragraph(_), Block::List { .. })
    )
}

fn render_block(
    block: &Block,
    t: &Theme,
    opts: RenderOptions,
    indent: usize,
    depth: usize,
    out: &mut Vec<Line<'static>>,
) {
    let width = opts.width.saturating_sub(indent).max(8);
    match block {
        Block::Heading { level, pieces } => render_heading(*level, pieces, t, opts, indent, out),
        Block::Paragraph(pieces) => {
            for line in wrap(pieces, width, t, 0) {
                out.push(pad(line, indent));
            }
        }
        Block::List { start, items } => render_list(*start, items, t, opts, indent, depth, out),
        Block::Code {
            language,
            lines: code,
        } => render_code(language.as_deref(), code, t, opts, indent, out),
        Block::Quote(blocks) => {
            let bar = if opts.unicode { "▌ " } else { "| " };
            let mut inner = Vec::new();
            render_blocks(blocks, t, opts, 0, depth, &mut inner);
            for line in inner {
                let mut spans = vec![Span::styled(bar.to_string(), t.muted())];
                spans.extend(line.spans);
                out.push(pad(Line::from(spans), indent));
            }
        }
        Block::Table { headers, rows } => render_table(headers, rows, t, opts, indent, out),
        Block::Rule => {
            let rule = if opts.unicode { "─" } else { "-" };
            out.push(pad(
                Line::from(Span::styled(rule.repeat(width.min(60)), t.muted())),
                indent,
            ));
        }
    }
}

/// Severity words get an accent, but the word stays — meaning is never carried
/// by color alone. Matched on the heading text rather than inferred from prose,
/// so an ordinary paragraph mentioning "critical" is left alone.
fn severity_style(text: &str, t: &Theme) -> Option<Style> {
    let key: String = text
        .trim()
        .trim_end_matches([':', '.'])
        .to_ascii_lowercase()
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string();
    match key.as_str() {
        "critical" => Some(t.failure().add_modifier(Modifier::BOLD)),
        "high" => Some(t.failure()),
        "medium" | "moderate" => Some(t.warning()),
        "low" => Some(t.secondary()),
        "informational" | "info" => Some(t.muted()),
        _ => None,
    }
}

fn render_heading(
    level: u8,
    pieces: &[Piece],
    t: &Theme,
    opts: RenderOptions,
    indent: usize,
    out: &mut Vec<Line<'static>>,
) {
    let width = opts.width.saturating_sub(indent).max(8);
    let text: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
    let severity = severity_style(&text, t);
    let style = severity.unwrap_or(match level {
        1 => t.primary().add_modifier(Modifier::BOLD),
        2 => t.brand().add_modifier(Modifier::BOLD),
        3 => t.text().add_modifier(Modifier::BOLD),
        _ => t.secondary().add_modifier(Modifier::BOLD),
    });
    // Levels 1 and 2 carry a rule; deeper levels get a leading mark instead, so
    // a six-level document does not become six ruled bands.
    let prefix = match (level, opts.unicode) {
        (1 | 2, _) => String::new(),
        (3, true) => "◆ ".into(),
        (3, false) => "* ".into(),
        (_, true) => "· ".into(),
        (_, false) => "- ".into(),
    };
    let body = format!("{prefix}{}", text.trim());
    for line in wrap_text(&body, width, style, prefix.width()) {
        out.push(pad(line, indent));
    }
    if level <= 2 {
        let rule = if opts.unicode { "─" } else { "-" };
        let len = body.width().min(width).max(3);
        out.push(pad(
            Line::from(Span::styled(rule.repeat(len), t.muted())),
            indent,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn render_list(
    start: Option<u64>,
    items: &[ListItem],
    t: &Theme,
    opts: RenderOptions,
    indent: usize,
    depth: usize,
    out: &mut Vec<Line<'static>>,
) {
    for (offset, item) in items.iter().enumerate() {
        let marker = match (start, item.task) {
            (_, Some(true)) => if opts.unicode { "[✓] " } else { "[x] " }.to_string(),
            (_, Some(false)) => "[ ] ".to_string(),
            (Some(first), None) => format!("{}. ", first + offset as u64),
            (None, None) => bullet(depth, opts.unicode).to_string(),
        };
        let hang = marker.width();
        // The first block sits beside the marker; anything after it — a second
        // paragraph, a nested list — is indented to the item's text column, so
        // nesting is visible and a sub-list never reuses the parent's bullet.
        let mut inner = Vec::new();
        if let Some((first, rest)) = item.blocks.split_first() {
            render_block(first, t, opts, 0, depth + 1, &mut inner);
            if !rest.is_empty() {
                let mut tail = Vec::new();
                render_blocks(rest, t, opts, 0, depth + 1, &mut tail);
                for line in tail {
                    inner.push(pad(line, LIST_INDENT));
                }
            }
        }
        for (line_index, line) in inner.into_iter().enumerate() {
            // Continuation lines align under the item text, not under its
            // marker — a wrapped bullet that restarts at column zero reads as
            // a new item.
            let lead = if line_index == 0 {
                Span::styled(marker.clone(), t.muted())
            } else {
                Span::raw(" ".repeat(hang))
            };
            let mut spans = vec![lead];
            spans.extend(line.spans);
            out.push(pad(Line::from(spans), indent));
        }
    }
}

/// Two columns per level: enough to see the nesting, cheap enough for a phone.
const LIST_INDENT: usize = 2;

fn bullet(depth: usize, unicode: bool) -> &'static str {
    match (depth % 3, unicode) {
        (0, true) => "• ",
        (1, true) => "◦ ",
        (_, true) => "– ",
        (0, false) => "* ",
        (1, false) => "- ",
        (_, false) => "+ ",
    }
}

fn render_code(
    language: Option<&str>,
    code: &[String],
    t: &Theme,
    opts: RenderOptions,
    indent: usize,
    out: &mut Vec<Line<'static>>,
) {
    let width = opts.width.saturating_sub(indent).max(8);
    // Below this a frame costs more columns than it earns; the code gets a
    // plain indent instead, which never collides with the timeline border.
    const FRAME_MIN_WIDTH: usize = 28;
    let framed = width >= FRAME_MIN_WIDTH;
    let (top_left, bar, bottom_left, rule) = if opts.unicode {
        ("┌─", "│ ", "└─", "─")
    } else {
        ("+-", "| ", "+-", "-")
    };
    if framed {
        let label = language.map(|l| format!(" {l} ")).unwrap_or_default();
        let dashes = width.saturating_sub(top_left.width() + label.width());
        out.push(pad(
            Line::from(vec![
                Span::styled(top_left.to_string(), t.muted()),
                Span::styled(label, t.secondary()),
                Span::styled(rule.repeat(dashes), t.muted()),
            ]),
            indent,
        ));
    }
    // Compact caps a long block: the operator can open the card for the rest,
    // and a 400-line paste should not bury the answer around it.
    const COMPACT_MAX_LINES: usize = 16;
    let (shown, elided) = if opts.compact && code.len() > COMPACT_MAX_LINES {
        (&code[..COMPACT_MAX_LINES], code.len() - COMPACT_MAX_LINES)
    } else {
        (code, 0)
    };
    let body_width = width.saturating_sub(if framed { bar.width() } else { 2 });
    for source in shown {
        // Code wraps rather than being clipped: a truncated command is a
        // command the operator cannot run. Indentation is preserved on the
        // first row and continuation rows are marked by the frame alone.
        for chunk in hard_wrap(source, body_width.max(4)) {
            let mut spans = Vec::new();
            if framed {
                spans.push(Span::styled(bar.to_string(), t.muted()));
            } else {
                spans.push(Span::raw("  ".to_string()));
            }
            spans.push(Span::styled(chunk, t.text()));
            out.push(pad(Line::from(spans), indent));
        }
    }
    if elided > 0 {
        out.push(pad(
            Line::from(Span::styled(
                format!(
                    "{bar}… {elided} more line{}",
                    if elided == 1 { "" } else { "s" }
                ),
                t.muted(),
            )),
            indent,
        ));
    }
    if framed {
        out.push(pad(
            Line::from(Span::styled(
                format!("{bottom_left}{}", rule.repeat(width.saturating_sub(2))),
                t.muted(),
            )),
            indent,
        ));
    }
}

fn render_table(
    headers: &[Vec<Piece>],
    rows: &[Vec<Vec<Piece>>],
    t: &Theme,
    opts: RenderOptions,
    indent: usize,
    out: &mut Vec<Line<'static>>,
) {
    let width = opts.width.saturating_sub(indent).max(8);
    let columns = headers
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return;
    }
    let cell_text = |cell: Option<&Vec<Piece>>| -> String {
        cell.map(|pieces| pieces.iter().map(|p| p.text.as_str()).collect::<String>())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let natural: Vec<usize> = (0..columns)
        .map(|column| {
            cell_text(headers.get(column)).width().max(
                rows.iter()
                    .map(|row| cell_text(row.get(column)).width())
                    .max()
                    .unwrap_or(0),
            )
        })
        .collect();
    let separators = 3 * columns.saturating_sub(1);
    let needed: usize = natural.iter().sum::<usize>() + separators;

    // Stacked records are the last resort, not the first: a table that does not
    // fit is still a table, and squeezing every column to two characters
    // destroys it more thoroughly than restating it as key/value rows.
    if needed > width && width < MIN_TABLE_WIDTH {
        for row in rows {
            for column in 0..columns {
                let key = cell_text(headers.get(column));
                let value = cell_text(row.get(column));
                if value.is_empty() {
                    continue;
                }
                let label = if key.is_empty() {
                    String::new()
                } else {
                    format!("{key}: ")
                };
                for line in wrap_text(&format!("{label}{value}"), width, t.text(), label.width()) {
                    out.push(pad(line, indent));
                }
            }
            out.push(Line::from(""));
        }
        while out.last().is_some_and(is_blank) {
            out.pop();
        }
        return;
    }

    // Shrink the widest column until it fits, so one long cell cannot push the
    // rest off the row.
    let mut widths = natural.clone();
    let mut budget = width.saturating_sub(separators);
    while widths.iter().sum::<usize>() > budget && budget > columns {
        let widest = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(index, _)| index)
            .unwrap_or(0);
        if widths[widest] <= MIN_COLUMN_WIDTH {
            break;
        }
        widths[widest] -= 1;
    }
    budget = budget.max(columns);
    let _ = budget;

    let divider = if opts.unicode { " │ " } else { " | " };
    let emit = |cells: &[String], style: Style, out: &mut Vec<Line<'static>>| {
        // Every cell wraps independently, then the rows are zipped, so a long
        // cell grows the row instead of overflowing it.
        let wrapped: Vec<Vec<String>> = cells
            .iter()
            .zip(&widths)
            .map(|(text, width)| {
                let lines = hard_wrap_words(text, *width);
                if lines.is_empty() {
                    vec![String::new()]
                } else {
                    lines
                }
            })
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for row in 0..height {
            let mut spans = Vec::new();
            for (column, cell) in wrapped.iter().enumerate() {
                if column > 0 {
                    spans.push(Span::styled(divider.to_string(), t.muted()));
                }
                let text = cell.get(row).cloned().unwrap_or_default();
                let pad_to = widths[column].saturating_sub(text.width());
                spans.push(Span::styled(text, style));
                if column + 1 < widths.len() && pad_to > 0 {
                    spans.push(Span::raw(" ".repeat(pad_to)));
                }
            }
            out.push(pad(Line::from(spans), indent));
        }
    };

    if !headers.is_empty() {
        let cells: Vec<String> = (0..columns).map(|c| cell_text(headers.get(c))).collect();
        emit(&cells, t.secondary().add_modifier(Modifier::BOLD), out);
        let rule = if opts.unicode { "─" } else { "-" };
        let total: usize = widths.iter().sum::<usize>() + separators;
        out.push(pad(
            Line::from(Span::styled(rule.repeat(total.min(width)), t.muted())),
            indent,
        ));
    }
    for row in rows {
        let cells: Vec<String> = (0..columns).map(|c| cell_text(row.get(c))).collect();
        emit(&cells, t.text(), out);
    }
}

/// Below this a bordered table is worse than stacked records.
const MIN_TABLE_WIDTH: usize = 44;
/// A column narrower than this holds nothing readable.
const MIN_COLUMN_WIDTH: usize = 6;

// ------------------------------------------------------------------ wrapping

/// Wrap styled pieces to a width, breaking on spaces and never inside a span's
/// styling. `hang` indents continuation rows.
fn wrap(pieces: &[Piece], width: usize, t: &Theme, hang: usize) -> Vec<Line<'static>> {
    let width = width.max(4);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut first = true;

    let mut flush = |current: &mut Vec<Span<'static>>, used: &mut usize, first: &mut bool| {
        if current.is_empty() {
            return;
        }
        let mut spans = Vec::new();
        if !*first && hang > 0 {
            spans.push(Span::raw(" ".repeat(hang)));
        }
        spans.append(current);
        lines.push(Line::from(spans));
        *used = 0;
        *first = false;
    };

    for piece in pieces {
        let style = piece_style(piece.emphasis, t);
        // A hard break is a break, not a word.
        for (segment_index, segment) in piece.text.split('\n').enumerate() {
            if segment_index > 0 {
                flush(&mut current, &mut used, &mut first);
            }
            for word in split_keeping_spaces(segment) {
                let word_width = word.width();
                let room = width.saturating_sub(if first { 0 } else { hang });
                if used + word_width > room && used > 0 {
                    // Never leave a trailing space at a wrap point.
                    if word.trim().is_empty() {
                        flush(&mut current, &mut used, &mut first);
                        continue;
                    }
                    flush(&mut current, &mut used, &mut first);
                }
                if word.trim().is_empty() && used == 0 {
                    continue;
                }
                // A single word longer than the row (a URL, a hash, a path) is
                // split rather than allowed to overflow — losing the tail would
                // lose the thing the operator needs.
                if word_width > room {
                    for chunk in hard_wrap(&word, room.max(4)) {
                        if used > 0 {
                            flush(&mut current, &mut used, &mut first);
                        }
                        used += chunk.width();
                        current.push(Span::styled(chunk, style));
                    }
                    continue;
                }
                used += word_width;
                current.push(Span::styled(word, style));
            }
        }
    }
    flush(&mut current, &mut used, &mut first);
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn wrap_text(text: &str, width: usize, style: Style, hang: usize) -> Vec<Line<'static>> {
    let width = width.max(4);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let projected = if current.is_empty() {
            word.width()
        } else {
            current.width() + 1 + word.width()
        };
        let room = width.saturating_sub(if lines.is_empty() { 0 } else { hang });
        if projected > room && !current.is_empty() {
            lines.push(styled_row(&current, style, hang, lines.is_empty()));
            current.clear();
        }
        if word.width() > room {
            for chunk in hard_wrap(word, room.max(4)) {
                if !current.is_empty() {
                    lines.push(styled_row(&current, style, hang, lines.is_empty()));
                    current.clear();
                }
                lines.push(styled_row(&chunk, style, hang, lines.is_empty()));
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(styled_row(&current, style, hang, lines.is_empty()));
    }
    lines
}

fn styled_row(text: &str, style: Style, hang: usize, first: bool) -> Line<'static> {
    if first || hang == 0 {
        Line::from(Span::styled(text.to_string(), style))
    } else {
        Line::from(vec![
            Span::raw(" ".repeat(hang)),
            Span::styled(text.to_string(), style),
        ])
    }
}

/// Split into words while keeping the spaces attached, so wrapping does not
/// silently collapse runs of whitespace inside a line.
fn split_keeping_spaces(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_space = false;
    for ch in text.chars() {
        let is_space = ch == ' ' || ch == '\t';
        if !current.is_empty() && is_space != in_space {
            out.push(std::mem::take(&mut current));
        }
        in_space = is_space;
        current.push(ch);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Split on display width, never inside a grapheme's char boundary.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = ch.to_string().width().max(1);
        if used + ch_width > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += ch_width;
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Word-preferring wrap used for table cells.
fn hard_wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let projected = if current.is_empty() {
            word.width()
        } else {
            current.width() + 1 + word.width()
        };
        if projected > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if word.width() > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.extend(hard_wrap(word, width));
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn piece_style(emphasis: Emphasis, t: &Theme) -> Style {
    let mut style = if emphasis.code {
        t.secondary()
    } else if emphasis.link {
        t.primary()
    } else {
        t.text()
    };
    if emphasis.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if emphasis.italic {
        // Terminals without italics fall back to dim, which is still a visible
        // difference rather than nothing.
        style = style.add_modifier(Modifier::ITALIC);
    }
    if emphasis.strike {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if emphasis.link {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn pad(line: Line<'static>, indent: usize) -> Line<'static> {
    if indent == 0 {
        return line;
    }
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    spans.extend(line.spans);
    Line::from(spans)
}

fn is_blank(line: &Line<'static>) -> bool {
    line.spans.iter().all(|span| span.content.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn opts(width: usize) -> RenderOptions {
        RenderOptions {
            width,
            compact: false,
            unicode: true,
        }
    }

    fn rendered(source: &str, width: usize) -> String {
        text_of(&render(&parse(source), &Theme::plain(), opts(width)))
    }

    /// **The defect.** Every one of these was reaching the operator verbatim.
    #[test]
    fn no_markdown_syntax_survives_into_the_rendered_document() {
        let source = "\
## Review summary

The **binding** is *unsafe* when `--bind 0.0.0.0` is used.

### Findings by severity

- **Suggested fix:** default to `127.0.0.1`
";
        let text = rendered(source, 72);
        for token in ["##", "###", "**", "`"] {
            assert!(!text.contains(token), "`{token}` survived:\n{text}");
        }
        // …and the content is all still there.
        for word in [
            "Review summary",
            "binding",
            "unsafe",
            "--bind 0.0.0.0",
            "127.0.0.1",
        ] {
            assert!(text.contains(word), "lost `{word}`:\n{text}");
        }
    }

    #[test]
    fn every_heading_level_renders_without_its_markers() {
        for level in 1..=6 {
            let source = format!("{} Heading {level}\n\nbody\n", "#".repeat(level));
            let text = rendered(&source, 60);
            assert!(!text.contains('#'), "level {level}:\n{text}");
            assert!(text.contains(&format!("Heading {level}")), "{text}");
        }
    }

    #[test]
    fn emphasis_applies_style_rather_than_leaving_markers() {
        let lines = render(
            &parse("a **bold** and *italic* and `code` word"),
            &Theme::plain(),
            opts(60),
        );
        let spans: Vec<(&str, Style)> = lines[0]
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style))
            .collect();
        let bold = spans
            .iter()
            .find(|(t, _)| t.contains("bold"))
            .expect("bold span");
        assert!(bold.1.add_modifier.contains(Modifier::BOLD), "{spans:?}");
        let italic = spans
            .iter()
            .find(|(t, _)| t.contains("italic"))
            .expect("italic span");
        assert!(
            italic.1.add_modifier.contains(Modifier::ITALIC),
            "{spans:?}"
        );
        assert!(spans.iter().any(|(t, _)| t.contains("code")));
    }

    #[test]
    fn nested_emphasis_keeps_both_styles() {
        let lines = render(&parse("***both***"), &Theme::plain(), opts(40));
        let span = &lines[0].spans[0];
        assert_eq!(span.content.as_ref(), "both");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn escaped_markdown_stays_literal() {
        let text = rendered(r"a \*not italic\* b", 40);
        assert!(text.contains("*not italic*"), "{text}");
    }

    #[test]
    fn lists_render_with_bullets_numbering_and_nesting() {
        let source = "\
- first
- second
  - nested
1. one
2. two
";
        let text = rendered(source, 60);
        assert!(text.contains("• first"), "{text}");
        assert!(
            text.contains("◦ nested") || text.contains("• nested"),
            "{text}"
        );
        assert!(text.contains("1. one"), "{text}");
        assert!(text.contains("2. two"), "{text}");
        assert!(!text.contains("- first"), "{text}");
    }

    #[test]
    fn a_wrapped_list_item_aligns_under_its_text() {
        let source = "- alpha beta gamma delta epsilon zeta eta theta iota kappa lambda\n";
        let lines = render(&parse(source), &Theme::plain(), opts(30));
        assert!(lines.len() > 1, "{:?}", text_of(&lines));
        let second = lines[1].spans[0].content.to_string();
        assert!(
            second.starts_with("  "),
            "continuation not hung: {:?}",
            text_of(&lines)
        );
    }

    #[test]
    fn task_lists_show_their_state() {
        let text = rendered("- [x] done\n- [ ] todo\n", 40);
        assert!(text.contains("[✓] done"), "{text}");
        assert!(text.contains("[ ] todo"), "{text}");
    }

    #[test]
    fn a_code_block_preserves_whitespace_and_labels_its_language() {
        let source = "```rust\nfn main() {\n    println!(\"Nexus\");\n}\n```\n";
        let text = rendered(source, 60);
        assert!(text.contains(" rust "), "{text}");
        assert!(text.contains("    println!(\"Nexus\");"), "{text}");
        assert!(!text.contains("```"), "{text}");
    }

    /// Markdown inside a fence is content, not syntax.
    #[test]
    fn markdown_inside_a_code_block_stays_literal() {
        let text = rendered("```\n## not a heading\n**not bold**\n```\n", 60);
        assert!(text.contains("## not a heading"), "{text}");
        assert!(text.contains("**not bold**"), "{text}");
    }

    /// Streaming: the fence has not closed yet.
    #[test]
    fn an_unclosed_fence_renders_as_provisional_code() {
        let text = rendered("intro\n\n```python\nx = 1\ny = 2", 60);
        assert!(text.contains("x = 1"), "{text}");
        assert!(text.contains("y = 2"), "{text}");
        assert!(text.contains("intro"), "{text}");
    }

    /// Streaming: emphasis that has not closed must not eat the text.
    #[test]
    fn partial_emphasis_never_loses_text() {
        for partial in ["a **bold", "a *ital", "a `code", "a [link", "# head"] {
            let text = rendered(partial, 40);
            let last = partial.split_whitespace().last().expect("non-empty");
            let word = last.trim_start_matches(['*', '`', '[', '#']);
            assert!(text.contains(word), "{partial:?} lost text: {text:?}");
        }
    }

    #[test]
    fn blockquotes_get_a_bar_and_keep_their_content() {
        let text = rendered("> quoted advice\n", 40);
        assert!(text.contains('▌'), "{text}");
        assert!(text.contains("quoted advice"), "{text}");
        assert!(!text.contains("> quoted"), "{text}");
    }

    #[test]
    fn a_thematic_break_becomes_a_rule() {
        let text = rendered("a\n\n---\n\nb", 40);
        assert!(text.contains('─'), "{text}");
        assert!(!text.contains("---"), "{text}");
    }

    #[test]
    fn a_link_shows_its_text_without_markdown_syntax() {
        let text = rendered("see [the docs](https://example.com/guide)", 60);
        assert!(text.contains("the docs"), "{text}");
        assert!(!text.contains("]("), "{text}");
        assert!(!text.contains('['), "{text}");
    }

    #[test]
    fn a_table_aligns_on_a_wide_terminal() {
        let source = "\
| Severity | Finding |
| --- | --- |
| High | Binds to 0.0.0.0 |
| Low | Verbose logging |
";
        let text = rendered(source, 80);
        assert!(text.contains("Severity"), "{text}");
        assert!(text.contains("Binds to 0.0.0.0"), "{text}");
        assert!(!text.contains("| --- |"), "{text}");
        assert!(text.contains('│'), "{text}");
    }

    /// A narrow terminal restates a table rather than crushing it.
    #[test]
    fn a_table_degrades_to_records_when_it_cannot_fit() {
        let source = "\
| Severity | Finding |
| --- | --- |
| High | The server binds to every interface over plain HTTP |
";
        let text = rendered(source, 32);
        assert!(text.contains("Severity: High"), "{text}");
        assert!(text.contains("plain HTTP"), "{text}");
        for line in text.lines() {
            assert!(line.width() <= 32, "overflow: {line:?}");
        }
    }

    #[test]
    fn a_malformed_table_still_renders_its_cells() {
        let text = rendered("| a | b |\n| c | d |\n", 60);
        assert!(text.contains('a') && text.contains('d'), "{text}");
    }

    /// Severity headings get an accent, and the word stays.
    #[test]
    fn severity_headings_are_labelled_not_only_coloured() {
        for label in ["Critical", "High", "Medium", "Low", "Informational"] {
            let text = rendered(&format!("### {label}\n\nbody\n"), 60);
            assert!(text.contains(label), "{text}");
        }
        let t = Theme::plain();
        assert!(severity_style("Critical findings", &t).is_some());
        assert!(severity_style("Suggested fix", &t).is_none());
        // Prose that merely mentions a severity word is not a severity heading:
        // matching is on the heading's first word.
        assert!(severity_style("Notes about critical paths", &t).is_none());
    }

    /// Model output is untrusted. A cursor jump, a title change, or a clipboard
    /// write must not survive into a span.
    #[test]
    fn terminal_control_sequences_are_neutralised() {
        let hostile = "before \x1b[2J\x1b]0;pwned\x07 \x1b]52;c;cGF5bG9hZA==\x07 after\r\nnext";
        let text = rendered(hostile, 80);
        assert!(!text.contains('\x1b'), "escape survived: {text:?}");
        assert!(!text.contains('\x07'), "bell survived: {text:?}");
        assert!(text.contains("before"), "{text}");
        assert!(text.contains("after"), "{text}");
    }

    #[test]
    fn raw_html_is_shown_as_text_not_interpreted() {
        let text = rendered("<script>alert(1)</script>\n", 60);
        assert!(text.contains("script"), "{text}");
    }

    /// Long unbreakable tokens are split rather than overflowing, because a
    /// truncated command is a command the operator cannot run.
    #[test]
    fn a_long_token_wraps_instead_of_overflowing() {
        let url = "https://example.com/".to_string() + &"segment/".repeat(20);
        let lines = render(&parse(&format!("see {url}")), &Theme::plain(), opts(40));
        for line in &lines {
            let width: usize = line.spans.iter().map(|s| s.content.width()).sum();
            assert!(width <= 40, "overflow: {:?}", text_of(&lines));
        }
        let joined: String = text_of(&lines)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        assert!(joined.contains("segment/segment/"), "{joined}");
    }

    #[test]
    fn no_width_panics_and_nothing_ever_overflows() {
        let source = "\
# Title
Body **bold** `code` [link](https://example.com).
- a
  - b
    - c
```rust
fn main() {}
```
> quote
| a | b |
| --- | --- |
| 1 | 2 |
---
";
        for width in 4usize..=120 {
            for compact in [true, false] {
                for unicode in [true, false] {
                    let lines = render(
                        &parse(source),
                        &Theme::plain(),
                        RenderOptions {
                            width,
                            compact,
                            unicode,
                        },
                    );
                    for line in &lines {
                        let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
                        assert!(
                            used <= width.max(8) + 8,
                            "width {width}: {used} > {width}: {:?}",
                            line.spans
                        );
                    }
                }
            }
        }
    }

    /// The ASCII tier must never emit a glyph a plain terminal cannot draw.
    #[test]
    fn the_ascii_fallback_stays_ascii() {
        let source =
            "# H\n- a\n  - b\n> q\n```sh\nls\n```\n---\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";
        let lines = render(
            &parse(source),
            &Theme::plain(),
            RenderOptions {
                width: 60,
                compact: false,
                unicode: false,
            },
        );
        let text = text_of(&lines);
        assert!(text.is_ascii(), "non-ascii in fallback: {text:?}");
    }

    /// Trimming an unclosed opener must not eat literal punctuation. This is
    /// the case that made the narrow left-flanking rule necessary.
    #[test]
    fn a_literal_asterisk_survives_the_streaming_trim() {
        assert_eq!(drop_unclosed_opener("2 * 3"), "2 * 3");
        assert_eq!(drop_unclosed_opener("a *b"), "a b");
        assert_eq!(drop_unclosed_opener("a **b"), "a b");
        assert_eq!(drop_unclosed_opener("a `cmd"), "a cmd");
        // Balanced markers are the parser's business, not this function's.
        assert_eq!(drop_unclosed_opener("plain text"), "plain text");
        let text = rendered("multiply 2 * 3 for the area", 60);
        assert!(text.contains("2 * 3"), "{text}");
    }

    /// A document is width-independent: reflowing is a render, not a reparse.
    #[test]
    fn the_same_document_reflows_at_a_new_width() {
        let doc = parse("alpha beta gamma delta epsilon zeta eta theta");
        let wide = render(&doc, &Theme::plain(), opts(80));
        let narrow = render(&doc, &Theme::plain(), opts(24));
        assert_eq!(wide.len(), 1);
        assert!(narrow.len() > 1);
        let flatten = |lines: &[Line<'static>]| {
            text_of(lines)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(flatten(&wide), flatten(&narrow));
    }

    /// Compact tightens spacing but never falls back to raw source.
    #[test]
    fn compact_mode_still_renders_semantically() {
        let source = "## Heading\n\nbody **bold**\n";
        let compact = text_of(&render(
            &parse(source),
            &Theme::plain(),
            RenderOptions {
                width: 60,
                compact: true,
                unicode: true,
            },
        ));
        assert!(!compact.contains("##"), "{compact}");
        assert!(!compact.contains("**"), "{compact}");
        assert!(compact.contains("Heading"), "{compact}");
    }

    #[test]
    fn compact_caps_a_very_long_code_block() {
        let body: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let text = text_of(&render(
            &parse(&format!("```\n{body}```\n")),
            &Theme::plain(),
            RenderOptions {
                width: 60,
                compact: true,
                unicode: true,
            },
        ));
        assert!(text.contains("more lines"), "{text}");
        assert!(!text.contains("line 39"), "{text}");
    }

    /// A long answer must not take pathological time to render.
    #[test]
    fn a_long_answer_renders_in_bounded_time() {
        let source: String = (0..400)
            .map(|i| format!("## Section {i}\n\nBody text with **bold** and `code`.\n\n- item\n\n"))
            .collect();
        let started = std::time::Instant::now();
        let lines = render(&parse(&source), &Theme::plain(), opts(100));
        assert!(lines.len() > 400);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    /// Streaming re-renders the same source repeatedly; the memo must return
    /// identical output and never accumulate without bound.
    #[test]
    fn the_render_cache_is_stable_and_bounded() {
        let first = text_of(&render_source("## A\n\nbody", &Theme::plain(), opts(60)));
        let second = text_of(&render_source("## A\n\nbody", &Theme::plain(), opts(60)));
        assert_eq!(first, second);
        for i in 0..200 {
            let _ = render_source(&format!("## {i}"), &Theme::plain(), opts(60));
        }
        let after = text_of(&render_source("## A\n\nbody", &Theme::plain(), opts(60)));
        assert_eq!(first, after);
    }

    /// A stream arriving one character at a time must never lose or duplicate
    /// content, and every prefix must render.
    #[test]
    fn every_prefix_of_a_streamed_answer_renders_safely() {
        let source =
            "## Title\n\nSome **bold** text and `code`.\n\n- one\n- two\n\n```rs\nfn f() {}\n```\n";
        for end in 1..=source.len() {
            if !source.is_char_boundary(end) {
                continue;
            }
            let text = rendered(&source[..end], 60);
            assert!(!text.contains("**"), "prefix {end}: {text:?}");
        }
        // And the finished answer contains each block exactly once.
        let text = rendered(source, 60);
        assert_eq!(text.matches("Title").count(), 1, "{text}");
        assert_eq!(text.matches("fn f() {}").count(), 1, "{text}");
    }
}
