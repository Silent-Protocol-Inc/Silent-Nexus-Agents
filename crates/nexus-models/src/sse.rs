//! Minimal Server-Sent-Events line assembler for streaming chat responses.
//!
//! Handles partial chunks: bytes are buffered until a complete line is
//! available; `data:` payloads are yielded; `[DONE]` terminates.

#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

pub enum SseItem {
    Data(String),
    Done,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes; returns zero or more complete `data:` payloads.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseItem> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=pos).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            let Some(data) = line.strip_prefix("data:") else {
                continue; // event:/id:/comments/blank lines
            };
            let data = data.trim_start();
            if data == "[DONE]" {
                out.push(SseItem::Done);
            } else if !data.is_empty() {
                out.push(SseItem::Data(data.to_string()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_split_lines() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: {\"a\":").is_empty());
        let items = p.feed(b"1}\n\ndata: [DONE]\n");
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], SseItem::Data(d) if d == "{\"a\":1}"));
        assert!(matches!(items[1], SseItem::Done));
    }

    #[test]
    fn ignores_comments_and_events() {
        let mut p = SseParser::new();
        let items = p.feed(b": keepalive\nevent: x\ndata: {}\n");
        assert_eq!(items.len(), 1);
    }
}
