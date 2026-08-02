//! Filesystem tool family (`fs.*`).
//!
//! All paths run through the workspace guard: canonicalized, symlink-checked,
//! and confined to the workspace. Reads are `Read` risk; mutations are
//! `Write`; deletion is `Destructive` and always requires confirmation.

use crate::{
    finalize_output, object_schema, Tool, ToolCategory, ToolContext, ToolMeta, ToolOutput,
    ToolRegistry,
};
use nexus_core::{NexusError, Result, RiskLevel};
use nexus_policy::ActionRequest;
use serde_json::{json, Value};
use sha2::Digest;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
enum FsOp {
    ListDir,
    ReadFile,
    SearchText,
    FindFiles,
    Stat,
    Hash,
    CreateFile,
    PatchFile,
    Move,
    Copy,
    Delete,
    Mkdir,
}

struct FsTool {
    meta: ToolMeta,
    op: FsOp,
}

fn meta(
    name: &str,
    description: &str,
    risk: RiskLevel,
    input_schema: Value,
    side_effects: &str,
) -> ToolMeta {
    ToolMeta {
        name: format!("fs.{name}"),
        namespace: "fs".into(),
        description: description.into(),
        category: ToolCategory::Filesystem,
        input_schema,
        output_schema: json!({"type": "string"}),
        risk,
        required_capabilities: vec!["filesystem".into()],
        timeout_secs: 30,
        max_output_bytes: 48_000,
        deterministic: true,
        needs_network: false,
        needs_sandbox: false,
        side_effects: side_effects.into(),
    }
}

pub fn register(registry: &mut ToolRegistry) {
    let tools: Vec<(FsOp, ToolMeta)> = vec![
        (
            FsOp::ListDir,
            meta(
                "list_dir",
                "List directory entries (name, type, size). Depth 1 by default.",
                RiskLevel::Read,
                object_schema(
                    &[],
                    &[
                        ("path", "string", "Directory path, workspace-relative; defaults to workspace root"),
                        ("depth", "integer", "Recursion depth (1-4), default 1"),
                    ],
                ),
                "none",
            ),
        ),
        (
            FsOp::ReadFile,
            meta(
                "read_file",
                "Read a file, optionally a line range. Output is line-numbered.",
                RiskLevel::Read,
                object_schema(
                    &[("path", "string", "File path, workspace-relative")],
                    &[
                        ("start_line", "integer", "First line (1-based, inclusive)"),
                        ("end_line", "integer", "Last line (inclusive)"),
                    ],
                ),
                "none",
            ),
        ),
        (
            FsOp::SearchText,
            meta(
                "search_text",
                "Regex search across workspace files (gitignore-aware). Returns path:line: matches.",
                RiskLevel::Read,
                object_schema(
                    &[("pattern", "string", "Rust-flavored regex")],
                    &[
                        ("path", "string", "Restrict to this subtree"),
                        ("max_results", "integer", "Cap results (default 50)"),
                    ],
                ),
                "none",
            ),
        ),
        (
            FsOp::FindFiles,
            meta(
                "find_files",
                "Find files by glob pattern, e.g. `**/*.rs` (gitignore-aware).",
                RiskLevel::Read,
                object_schema(
                    &[("glob", "string", "Glob pattern")],
                    &[("path", "string", "Restrict to this subtree")],
                ),
                "none",
            ),
        ),
        (
            FsOp::Stat,
            meta(
                "stat",
                "File metadata: size, modified time, type, permissions.",
                RiskLevel::Read,
                object_schema(&[("path", "string", "Path to inspect")], &[]),
                "none",
            ),
        ),
        (
            FsOp::Hash,
            meta(
                "hash",
                "SHA-256 hash of a file.",
                RiskLevel::Read,
                object_schema(&[("path", "string", "File to hash")], &[]),
                "none",
            ),
        ),
        (
            FsOp::CreateFile,
            meta(
                "create_file",
                "Create a new file (or overwrite an existing one) with the given content.",
                RiskLevel::Write,
                object_schema(
                    &[
                        ("path", "string", "Target path"),
                        ("content", "string", "Full file content"),
                    ],
                    &[("overwrite", "boolean", "Allow replacing an existing file (default false)")],
                ),
                "creates or replaces one file inside the workspace",
            ),
        ),
        (
            FsOp::PatchFile,
            meta(
                "patch_file",
                "Edit a file by exact text replacement. `old_text` must occur exactly once (or set replace_all). Whitespace-sensitive.",
                RiskLevel::Write,
                object_schema(
                    &[
                        ("path", "string", "File to edit"),
                        ("old_text", "string", "Exact existing text to replace"),
                        ("new_text", "string", "Replacement text"),
                    ],
                    &[("replace_all", "boolean", "Replace every occurrence (default false)")],
                ),
                "modifies one file in place; original content recoverable via git or artifact backup",
            ),
        ),
        (
            FsOp::Move,
            meta(
                "move",
                "Move or rename a file/directory within the workspace.",
                RiskLevel::Write,
                object_schema(
                    &[
                        ("from", "string", "Source path"),
                        ("to", "string", "Destination path"),
                    ],
                    &[],
                ),
                "relocates a path inside the workspace",
            ),
        ),
        (
            FsOp::Copy,
            meta(
                "copy",
                "Copy a file within the workspace.",
                RiskLevel::Write,
                object_schema(
                    &[
                        ("from", "string", "Source file"),
                        ("to", "string", "Destination path"),
                    ],
                    &[],
                ),
                "creates a copy inside the workspace",
            ),
        ),
        (
            FsOp::Delete,
            meta(
                "delete",
                "Delete a file or (with recursive=true) a directory. Always requires confirmation.",
                RiskLevel::Destructive,
                object_schema(
                    &[("path", "string", "Path to delete")],
                    &[("recursive", "boolean", "Required to delete non-empty directories")],
                ),
                "permanently removes data (no trash)",
            ),
        ),
        (
            FsOp::Mkdir,
            meta(
                "mkdir",
                "Create a directory (with parents).",
                RiskLevel::Write,
                object_schema(&[("path", "string", "Directory to create")], &[]),
                "creates directories inside the workspace",
            ),
        ),
    ];
    for (op, m) in tools {
        registry.register(Arc::new(FsTool { meta: m, op }));
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| NexusError::ToolInput {
            tool: "fs".into(),
            message: format!("missing string argument `{key}`"),
        })
}

#[async_trait::async_trait]
impl Tool for FsTool {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn action_request(&self, args: &Value) -> Result<ActionRequest> {
        let mut paths = Vec::new();
        for key in ["path", "from", "to"] {
            if let Some(p) = args.get(key).and_then(Value::as_str) {
                paths.push(p.to_string());
            }
        }
        let summary = match self.op {
            FsOp::Delete => format!(
                "DELETE {}{}",
                paths.first().map(String::as_str).unwrap_or("?"),
                if args
                    .get("recursive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    " (recursive)"
                } else {
                    ""
                }
            ),
            FsOp::CreateFile => format!(
                "create file {} ({} bytes)",
                paths.first().map(String::as_str).unwrap_or("?"),
                args.get("content")
                    .and_then(Value::as_str)
                    .map(str::len)
                    .unwrap_or(0)
            ),
            FsOp::PatchFile => format!(
                "edit {} (replace {} chars)",
                paths.first().map(String::as_str).unwrap_or("?"),
                args.get("old_text")
                    .and_then(Value::as_str)
                    .map(str::len)
                    .unwrap_or(0)
            ),
            _ => format!("{} {}", self.meta.name, paths.join(" → ")),
        };
        Ok(ActionRequest {
            tool: self.meta.name.clone(),
            risk: self.meta.risk,
            paths,
            formats: Vec::new(),
            command: None,
            command_analysis: None,
            destination: None,
            summary,
        })
    }

    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolOutput> {
        let guard = ctx.workspace.clone();
        let policy = ctx.config.policy.clone();
        let op = self.op;
        // Filesystem work is synchronous; run on the blocking pool.
        let raw = tokio::task::spawn_blocking(move || execute_sync(op, &guard, &policy, &args))
            .await
            .map_err(|e| NexusError::other(format!("fs task join: {e}")))??;
        finalize_output(ctx, &self.meta, raw.0, raw.1).await
    }
}

fn execute_sync(
    op: FsOp,
    guard: &nexus_core::workspace::WorkspaceGuard,
    policy: &nexus_core::config::PolicyConfig,
    args: &Value,
) -> Result<(String, Value)> {
    match op {
        FsOp::ListDir => {
            let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let depth = args
                .get("depth")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 4) as usize;
            let root = guard.resolve_existing(rel)?;
            let mut lines = Vec::new();
            let mut skipped = 0usize;
            let walker = ignore::WalkBuilder::new(&root)
                .max_depth(Some(depth))
                .hidden(false)
                .git_ignore(true)
                .build();
            for entry in walker.flatten() {
                let path = entry.path();
                if path == root {
                    continue;
                }
                let Ok(real) = guard.resolve_existing(path) else {
                    continue;
                };
                if entry.file_type().is_some_and(|kind| kind.is_file())
                    && read_format_decision(guard, policy, &real) == "deny"
                {
                    skipped += 1;
                    continue;
                }
                let ft = entry.file_type();
                let kind = if ft.map(|t| t.is_dir()).unwrap_or(false) {
                    "dir "
                } else {
                    "file"
                };
                let size = std::fs::metadata(&real).map(|m| m.len()).unwrap_or(0);
                lines.push(format!(
                    "{kind} {:>9}  {}",
                    size,
                    guard.display_relative(&real)
                ));
            }
            lines.sort();
            let count = lines.len();
            Ok((
                lines.join("\n"),
                json!({"entries": count, "skipped_restricted": skipped}),
            ))
        }
        FsOp::ReadFile => {
            let path = guard.resolve_existing(arg_str(args, "path")?)?;
            ensure_readable(guard, policy, &path)?;
            let content = std::fs::read_to_string(&path).map_err(|e| NexusError::ToolFailed {
                tool: "fs.read_file".into(),
                message: format!("{}: {e}", path.display()),
            })?;
            let start = args.get("start_line").and_then(Value::as_u64).unwrap_or(1) as usize;
            let end = args
                .get("end_line")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX) as usize;
            let total = content.lines().count();
            let numbered: Vec<String> = content
                .lines()
                .enumerate()
                .filter(|(i, _)| {
                    let n = i + 1;
                    n >= start && n <= end
                })
                .map(|(i, l)| format!("{:>5} | {l}", i + 1))
                .collect();
            Ok((
                numbered.join("\n"),
                json!({"total_lines": total, "shown": numbered.len()}),
            ))
        }
        FsOp::SearchText => {
            let pattern = arg_str(args, "pattern")?;
            let re = regex::Regex::new(pattern).map_err(|e| NexusError::ToolInput {
                tool: "fs.search_text".into(),
                message: format!("invalid regex: {e}"),
            })?;
            let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let max = args
                .get("max_results")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .clamp(1, 500) as usize;
            let root = guard.resolve_existing(rel)?;
            let mut hits = Vec::new();
            let mut skipped = 0usize;
            let walker = ignore::WalkBuilder::new(&root)
                .hidden(false)
                .git_ignore(true)
                .build();
            'outer: for entry in walker.flatten() {
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                // Guard against reading denied paths inside the walk.
                let Ok(real) = guard.resolve_existing(entry.path()) else {
                    continue;
                };
                if read_format_decision(guard, policy, &real) == "deny" {
                    skipped += 1;
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&real) else {
                    continue; // binary or unreadable
                };
                for (i, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        let display = line.trim();
                        let display = if display.len() > 240 {
                            &display[..display
                                .char_indices()
                                .take(240)
                                .last()
                                .map(|(i, c)| i + c.len_utf8())
                                .unwrap_or(240)]
                        } else {
                            display
                        };
                        hits.push(format!(
                            "{}:{}: {display}",
                            guard.display_relative(&real),
                            i + 1,
                        ));
                        if hits.len() >= max {
                            break 'outer;
                        }
                    }
                }
            }
            let count = hits.len();
            Ok((
                if hits.is_empty() {
                    format!("no matches for `{pattern}`")
                } else {
                    hits.join("\n")
                },
                json!({"matches": count, "capped": count >= max, "skipped_restricted": skipped}),
            ))
        }
        FsOp::FindFiles => {
            let glob = arg_str(args, "glob")?;
            let matcher = globset::GlobBuilder::new(glob)
                .literal_separator(false)
                .build()
                .map_err(|e| NexusError::ToolInput {
                    tool: "fs.find_files".into(),
                    message: format!("invalid glob: {e}"),
                })?
                .compile_matcher();
            let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
            let root = guard.resolve_existing(rel)?;
            let mut found = Vec::new();
            let mut skipped = 0usize;
            let walker = ignore::WalkBuilder::new(&root)
                .hidden(false)
                .git_ignore(true)
                .build();
            for entry in walker.flatten() {
                let Ok(real) = guard.resolve_existing(entry.path()) else {
                    continue;
                };
                if entry.file_type().is_some_and(|kind| kind.is_file())
                    && read_format_decision(guard, policy, &real) == "deny"
                {
                    skipped += 1;
                    continue;
                }
                let rel_path = guard.display_relative(&real);
                if matcher.is_match(&rel_path) {
                    found.push(rel_path);
                }
                if found.len() >= 500 {
                    break;
                }
            }
            found.sort();
            let count = found.len();
            Ok((
                if found.is_empty() {
                    format!("no files match `{glob}`")
                } else {
                    found.join("\n")
                },
                json!({"files": count, "skipped_restricted": skipped}),
            ))
        }
        FsOp::Stat => {
            let path = guard.resolve_existing(arg_str(args, "path")?)?;
            ensure_readable(guard, policy, &path)?;
            let m = std::fs::metadata(&path)?;
            let modified = m
                .modified()
                .ok()
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs())
                })
                .map(|secs| {
                    chrono::DateTime::from_timestamp(secs as i64, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let kind = if m.is_dir() { "directory" } else { "file" };
            Ok((
                format!(
                    "{}: {kind}, {} bytes, modified {modified}",
                    guard.display_relative(&path),
                    m.len()
                ),
                json!({"size": m.len(), "is_dir": m.is_dir(), "modified": modified}),
            ))
        }
        FsOp::Hash => {
            let path = guard.resolve_existing(arg_str(args, "path")?)?;
            ensure_readable(guard, policy, &path)?;
            let bytes = std::fs::read(&path)?;
            let hash = hex::encode(sha2::Sha256::digest(&bytes));
            Ok((
                format!("sha256({}) = {hash}", guard.display_relative(&path)),
                json!({"sha256": hash, "bytes": bytes.len()}),
            ))
        }
        FsOp::CreateFile => {
            let rel = arg_str(args, "path")?;
            let content = arg_str(args, "content")?;
            let overwrite = args
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let path = guard.resolve_for_write(rel)?;
            if path.exists() && !overwrite {
                return Err(NexusError::ToolFailed {
                    tool: "fs.create_file".into(),
                    message: format!(
                        "{rel} already exists; pass overwrite=true or use fs.patch_file"
                    ),
                });
            }
            let prior = if path.exists() {
                std::fs::read_to_string(&path).unwrap_or_default()
            } else {
                String::new()
            };
            nexus_core::atomic::atomic_write(&path, content.as_bytes(), file_mode(&path))?;
            let rel_display = guard.display_relative(&path);
            Ok((
                format!("wrote {} ({} bytes)", rel_display, content.len()),
                json!({
                    "bytes": content.len(),
                    "diff": mutation_diff(&rel_display, &prior, content),
                }),
            ))
        }
        FsOp::PatchFile => {
            let rel = arg_str(args, "path")?;
            let old_text = arg_str(args, "old_text")?;
            let new_text = arg_str(args, "new_text")?;
            let replace_all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if old_text.is_empty() {
                return Err(NexusError::ToolInput {
                    tool: "fs.patch_file".into(),
                    message: "old_text may not be empty".into(),
                });
            }
            let path = guard.resolve_existing(rel)?;
            let content = std::fs::read_to_string(&path)?;
            let occurrences = content.matches(old_text).count();
            if occurrences == 0 {
                return Err(NexusError::ToolFailed {
                    tool: "fs.patch_file".into(),
                    message: format!(
                        "old_text not found in {rel}; re-read the file — content may have changed"
                    ),
                });
            }
            if occurrences > 1 && !replace_all {
                return Err(NexusError::ToolFailed {
                    tool: "fs.patch_file".into(),
                    message: format!(
                        "old_text occurs {occurrences} times in {rel}; add more context or set replace_all=true"
                    ),
                });
            }
            let updated = if replace_all {
                content.replace(old_text, new_text)
            } else {
                content.replacen(old_text, new_text, 1)
            };
            nexus_core::atomic::atomic_write(&path, updated.as_bytes(), file_mode(&path))?;
            let rel_display = guard.display_relative(&path);
            Ok((
                format!(
                    "patched {} ({} replacement{})",
                    rel_display,
                    occurrences.min(if replace_all { occurrences } else { 1 }),
                    if replace_all && occurrences > 1 {
                        "s"
                    } else {
                        ""
                    }
                ),
                json!({
                    "replacements": if replace_all { occurrences } else { 1 },
                    "diff": mutation_diff(&rel_display, old_text, new_text),
                }),
            ))
        }
        FsOp::Move => {
            let from = guard.resolve_existing(arg_str(args, "from")?)?;
            let destination = arg_str(args, "to")?;
            let to = guard.resolve_for_write(destination)?;
            if let Some(parent) = to.parent() {
                nexus_core::atomic::ensure_directory_tree(parent, 0o755)?;
            }
            let to = guard.resolve_for_write(destination)?;
            std::fs::rename(&from, &to)?;
            let to_display = guard.display_relative(&to);
            Ok((
                format!("moved {} → {}", guard.display_relative(&from), to_display),
                json!({
                    "diff": {
                        "path": to_display,
                        "insertions": 0,
                        "deletions": 0,
                        "preview": "",
                    }
                }),
            ))
        }
        FsOp::Copy => {
            let from = guard.resolve_existing(arg_str(args, "from")?)?;
            let to = guard.resolve_for_write(arg_str(args, "to")?)?;
            if from.is_dir() {
                return Err(NexusError::ToolFailed {
                    tool: "fs.copy".into(),
                    message: "directory copy not supported; copy files individually".into(),
                });
            }
            let content = std::fs::read(&from)?;
            nexus_core::atomic::atomic_write(&to, &content, file_mode(&from))?;
            let bytes = content.len() as u64;
            Ok((
                format!(
                    "copied {} → {} ({bytes} bytes)",
                    guard.display_relative(&from),
                    guard.display_relative(&to)
                ),
                json!({"bytes": bytes}),
            ))
        }
        FsOp::Delete => {
            let path = guard.resolve_existing(arg_str(args, "path")?)?;
            let recursive = args
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            // Refuse to delete the workspace root itself.
            if path == guard.root() {
                return Err(NexusError::PathDenied(
                    "refusing to delete the workspace root".into(),
                ));
            }
            // Capture text content before removal so the timeline can show the
            // removed lines (`-`). Best-effort: directories and unreadable/binary
            // files simply produce a path-only diff card.
            let removed = if path.is_dir() {
                String::new()
            } else {
                std::fs::read_to_string(&path).unwrap_or_default()
            };
            if path.is_dir() {
                if recursive {
                    std::fs::remove_dir_all(&path)?;
                } else {
                    std::fs::remove_dir(&path).map_err(|e| NexusError::ToolFailed {
                        tool: "fs.delete".into(),
                        message: format!(
                            "{e}; directory not empty? set recursive=true to delete contents"
                        ),
                    })?;
                }
            } else {
                std::fs::remove_file(&path)?;
            }
            let rel_display = guard.display_relative(&path);
            Ok((
                format!("deleted {}", rel_display),
                json!({"diff": mutation_diff(&rel_display, &removed, "")}),
            ))
        }
        FsOp::Mkdir => {
            let path = guard.resolve_for_write(arg_str(args, "path")?)?;
            nexus_core::atomic::ensure_directory_tree(&path, 0o755)?;
            Ok((
                format!("created directory {}", guard.display_relative(&path)),
                Value::Null,
            ))
        }
    }
}

fn read_format_decision<'a>(
    guard: &nexus_core::workspace::WorkspaceGuard,
    policy: &'a nexus_core::config::PolicyConfig,
    path: &std::path::Path,
) -> &'a str {
    // A session that has answered full access's one question has already been
    // asked about exactly these paths. Refusing here anyway is how the operator
    // approved a read and was then told the file did not exist — the listing
    // hid it too, so the model never even attempted it.
    if guard.denied_paths_unlocked() {
        return "allow";
    }
    let classified = nexus_core::file_formats::classify(path);
    if classified.hard_denied {
        return "deny";
    }
    policy
        .read_formats
        .get(classified.id)
        .or_else(|| policy.read_formats.get("other"))
        .map(String::as_str)
        .unwrap_or(policy.reads.as_str())
}

fn ensure_readable(
    guard: &nexus_core::workspace::WorkspaceGuard,
    policy: &nexus_core::config::PolicyConfig,
    path: &std::path::Path,
) -> Result<()> {
    if read_format_decision(guard, policy, path) == "deny" {
        return Err(NexusError::PathDenied(format!(
            "read denied for `{}` (format {})",
            path.display(),
            nexus_core::file_formats::classify(path).id
        )));
    }
    Ok(())
}

/// UI-only change summary attached to a mutating op's metadata.
///
/// Represents `old` → `new` as removed (`-`) then added (`+`) lines so the
/// timeline can render a highlighted diff and the file path even for a freshly
/// created file (all `+`) or a deleted one (all `-`). This never reaches the
/// model — only `ToolOutput.content` does.
fn mutation_diff(path_rel: &str, old: &str, new: &str) -> Value {
    const MAX_LINES: usize = 200;
    let insertions = if new.is_empty() {
        0
    } else {
        new.lines().count()
    };
    let deletions = if old.is_empty() {
        0
    } else {
        old.lines().count()
    };
    let mut preview = String::new();
    for line in old.lines().take(MAX_LINES) {
        preview.push('-');
        preview.push_str(line);
        preview.push('\n');
    }
    for line in new.lines().take(MAX_LINES) {
        preview.push('+');
        preview.push_str(line);
        preview.push('\n');
    }
    json!({
        "path": path_rel,
        "insertions": insertions,
        "deletions": deletions,
        "preview": preview.trim_end(),
    })
}

fn file_mode(path: &std::path::Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or(0o644)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o644
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::context;
    use crate::ToolRegistry;

    fn registry() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        register(&mut r);
        r
    }

    async fn run(
        ctx: &ToolContext,
        r: &ToolRegistry,
        name: &str,
        args: Value,
    ) -> Result<ToolOutput> {
        r.validate_args(name, &args)?;
        r.get(name)?.execute(ctx, args).await
    }

    #[tokio::test]
    async fn read_write_patch_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "a.txt", "content": "hello world\n"}),
        )
        .await
        .expect("create");
        let out = run(&ctx, &r, "fs.read_file", json!({"path": "a.txt"}))
            .await
            .expect("read");
        assert!(out.content.contains("hello world"));
        assert!(out.content.contains("1 |"));
        run(
            &ctx,
            &r,
            "fs.patch_file",
            json!({"path": "a.txt", "old_text": "hello", "new_text": "goodbye"}),
        )
        .await
        .expect("patch");
        let out = run(&ctx, &r, "fs.read_file", json!({"path": "a.txt"}))
            .await
            .expect("read2");
        assert!(out.content.contains("goodbye world"));
    }

    #[tokio::test]
    async fn patch_requires_unique_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "b.txt", "content": "x\nx\n"}),
        )
        .await
        .expect("create");
        let err = run(
            &ctx,
            &r,
            "fs.patch_file",
            json!({"path": "b.txt", "old_text": "x", "new_text": "y"}),
        )
        .await
        .expect_err("ambiguous");
        assert!(err.to_string().contains("2 times"));
    }

    #[tokio::test]
    async fn traversal_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        let err = run(
            &ctx,
            &r,
            "fs.read_file",
            json!({"path": "../../etc/passwd"}),
        )
        .await
        .expect_err("must fail");
        assert!(matches!(
            err,
            NexusError::PathEscape(_) | NexusError::NotFound(_)
        ));
        let err = run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "/etc/snx_evil", "content": "x"}),
        )
        .await
        .expect_err("must fail");
        assert!(matches!(err, NexusError::PathEscape(_)));
    }

    #[tokio::test]
    async fn delete_refuses_workspace_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        let err = run(&ctx, &r, "fs.delete", json!({"path": "."}))
            .await
            .expect_err("must fail");
        assert!(matches!(err, NexusError::PathDenied(_)));
    }

    #[tokio::test]
    async fn search_and_find() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "src/needle.rs", "content": "fn special_needle() {}\n"}),
        )
        .await
        .expect("create");
        let out = run(
            &ctx,
            &r,
            "fs.search_text",
            json!({"pattern": "special_needle"}),
        )
        .await
        .expect("search");
        assert!(out.content.contains("src/needle.rs:1"));
        let out = run(&ctx, &r, "fs.find_files", json!({"glob": "**/*.rs"}))
            .await
            .expect("find");
        assert!(out.content.contains("src/needle.rs"));
    }

    #[tokio::test]
    async fn listing_and_discovery_hide_denied_paths() {
        let directory = tempfile::tempdir().expect("directory");
        let ctx = context(directory.path());
        std::fs::create_dir_all(directory.path().join(".git")).expect("git");
        std::fs::write(directory.path().join(".git/config"), "secret").expect("git config");
        std::fs::write(directory.path().join(".nexus/state/private"), "secret").expect("state");
        std::fs::write(directory.path().join(".env"), "TOKEN=secret").expect("env");
        std::fs::write(directory.path().join(".env.example"), "TOKEN=").expect("example");
        std::fs::write(directory.path().join("visible.txt"), "visible").expect("visible");
        let registry = registry();

        let listed = run(
            &ctx,
            &registry,
            "fs.list_dir",
            json!({"path": ".", "depth": 3}),
        )
        .await
        .expect("list");
        assert!(listed.content.contains("visible.txt"));
        assert!(!listed.content.contains(".env.example"));
        assert!(!listed.content.contains(".env\n"));
        assert!(!listed.content.contains(".git"));
        assert!(!listed.content.contains(".nexus"));

        let found = run(
            &ctx,
            &registry,
            "fs.find_files",
            json!({"glob": "**/*", "path": "."}),
        )
        .await
        .expect("find");
        assert!(found.content.contains("visible.txt"));
        assert!(!found.content.contains(".env.example"));
        assert!(!found.content.lines().any(|line| line == ".env"));
        assert!(!found.content.contains(".git"));
        assert!(!found.content.contains(".nexus"));
    }

    #[tokio::test]
    async fn delete_action_request_is_destructive() {
        let r = registry();
        let tool = r.get("fs.delete").expect("tool");
        let req = tool
            .action_request(&json!({"path": "src", "recursive": true}))
            .expect("action");
        assert_eq!(req.risk, RiskLevel::Destructive);
        assert!(req.summary.contains("recursive"));
    }

    #[tokio::test]
    async fn create_file_emits_added_lines_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        let out = run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "page.html", "content": "<html>\n<body>hi</body>\n</html>\n"}),
        )
        .await
        .expect("create");
        let diff = out.metadata.get("diff").expect("diff metadata");
        assert_eq!(diff.get("path").and_then(Value::as_str), Some("page.html"));
        assert_eq!(diff.get("insertions").and_then(Value::as_u64), Some(3));
        assert_eq!(diff.get("deletions").and_then(Value::as_u64), Some(0));
        let preview = diff
            .get("preview")
            .and_then(Value::as_str)
            .expect("preview");
        assert!(preview.lines().all(|line| line.starts_with('+')));
        assert!(preview.contains("+<body>hi</body>"));
    }

    #[tokio::test]
    async fn patch_file_emits_removed_and_added_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "c.txt", "content": "alpha\n"}),
        )
        .await
        .expect("create");
        let out = run(
            &ctx,
            &r,
            "fs.patch_file",
            json!({"path": "c.txt", "old_text": "alpha", "new_text": "beta"}),
        )
        .await
        .expect("patch");
        let diff = out.metadata.get("diff").expect("diff metadata");
        assert_eq!(diff.get("insertions").and_then(Value::as_u64), Some(1));
        assert_eq!(diff.get("deletions").and_then(Value::as_u64), Some(1));
        let preview = diff
            .get("preview")
            .and_then(Value::as_str)
            .expect("preview");
        assert!(preview.contains("-alpha"));
        assert!(preview.contains("+beta"));
    }

    #[tokio::test]
    async fn delete_emits_removed_lines_diff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let r = registry();
        run(
            &ctx,
            &r,
            "fs.create_file",
            json!({"path": "gone.html", "content": "one\ntwo\n"}),
        )
        .await
        .expect("create");
        let out = run(&ctx, &r, "fs.delete", json!({"path": "gone.html"}))
            .await
            .expect("delete");
        let diff = out.metadata.get("diff").expect("diff metadata");
        assert_eq!(diff.get("path").and_then(Value::as_str), Some("gone.html"));
        assert_eq!(diff.get("insertions").and_then(Value::as_u64), Some(0));
        assert_eq!(diff.get("deletions").and_then(Value::as_u64), Some(2));
        let preview = diff
            .get("preview")
            .and_then(Value::as_str)
            .expect("preview");
        assert!(preview.lines().all(|line| line.starts_with('-')));
        assert!(preview.contains("-two"));
    }
}
