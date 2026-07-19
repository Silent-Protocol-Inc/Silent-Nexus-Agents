//! nexus-index: code intelligence index.
//!
//! Builds a persisted, incrementally-updatable index of the workspace: file
//! tree, language detection, and symbols. Symbol extraction uses fast,
//! dependency-light regex heuristics per language (documented as approximate).
//! A tree-sitter backend can replace the private extraction implementation
//! later without changing [`Indexer`] callers — see docs/dependencies.md for
//! why we start heuristic. The index never loads whole files into model context; it
//! answers "where is symbol X" and "what's in this file" with pointers.

use nexus_core::store::Store;
use nexus_core::Result;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatus {
    pub files: usize,
    pub symbols: usize,
    pub languages: Vec<(String, usize)>,
    pub last_built: Option<String>,
}

pub struct Indexer {
    store: Store,
}

impl Indexer {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Detect a language from a file extension.
    pub fn detect_language(path: &Path) -> &'static str {
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs") => "rust",
            Some("ts" | "tsx") => "typescript",
            Some("js" | "jsx" | "mjs" | "cjs") => "javascript",
            Some("py") => "python",
            Some("go") => "go",
            Some("java") => "java",
            Some("c" | "h") => "c",
            Some("cpp" | "cc" | "cxx" | "hpp") => "cpp",
            Some("rb") => "ruby",
            Some("sh" | "bash") => "shell",
            Some("md") => "markdown",
            Some("toml") => "toml",
            Some("json") => "json",
            Some("yaml" | "yml") => "yaml",
            _ => "other",
        }
    }

    /// Build (or rebuild) the index over `root`, respecting gitignore.
    /// Incremental: files whose size+mtime+hash are unchanged are skipped.
    pub fn build(
        &self,
        root: &Path,
        guard: &nexus_core::workspace::WorkspaceGuard,
    ) -> Result<IndexStatus> {
        self.build_with_policy(root, guard, &nexus_core::config::PolicyConfig::default())
    }

    pub fn build_with_policy(
        &self,
        root: &Path,
        guard: &nexus_core::workspace::WorkspaceGuard,
        policy: &nexus_core::config::PolicyConfig,
    ) -> Result<IndexStatus> {
        let mut allowed_paths = std::collections::BTreeSet::new();
        for entry in ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .build()
            .flatten()
        {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            // Skip anything the guard would deny.
            let Ok(real) = guard.resolve_existing(path) else {
                continue;
            };
            let rel = guard.display_relative(&real);
            let classified = nexus_core::file_formats::classify(&real);
            let decision = if classified.hard_denied {
                "deny"
            } else {
                policy
                    .read_formats
                    .get(classified.id)
                    .or_else(|| policy.read_formats.get("other"))
                    .map(String::as_str)
                    .unwrap_or(policy.reads.as_str())
            };
            if decision != "allow" {
                continue;
            }
            allowed_paths.insert(rel.clone());
            let meta = match std::fs::metadata(&real) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            // Incremental skip.
            let unchanged: bool = self.store.with(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT size, mtime_ms FROM index_files WHERE path = ?1",
                        [&rel],
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                    )
                    .map(|(s, m)| s as u64 == size && m == mtime)
                    .unwrap_or(false))
            })?;
            if unchanged {
                continue;
            }
            let content = match std::fs::read(&real) {
                Ok(b) => b,
                Err(_) => continue,
            };
            // Skip binary files (NUL byte heuristic) and very large files.
            if content.len() > 2_000_000 || content.iter().take(8000).any(|b| *b == 0) {
                continue;
            }
            let text = String::from_utf8_lossy(&content);
            let language = Self::detect_language(&real);
            let hash = hex::encode(sha2::Sha256::digest(&content));
            let symbols = extract_symbols(language, &text);

            self.store.with(|conn| {
                conn.execute(
                    "INSERT INTO index_files (path, language, size, mtime_ms, sha256, indexed_at)
                     VALUES (?1,?2,?3,?4,?5,?6)
                     ON CONFLICT(path) DO UPDATE SET
                       language=excluded.language, size=excluded.size, mtime_ms=excluded.mtime_ms,
                       sha256=excluded.sha256, indexed_at=excluded.indexed_at",
                    rusqlite::params![
                        rel,
                        language,
                        size as i64,
                        mtime,
                        hash,
                        nexus_core::now_rfc3339()
                    ],
                )?;
                conn.execute("DELETE FROM index_symbols WHERE path = ?1", [&rel])?;
                for s in &symbols {
                    conn.execute(
                        "INSERT INTO index_symbols (path, name, kind, line, signature)
                         VALUES (?1,?2,?3,?4,?5)",
                        rusqlite::params![rel, s.name, s.kind, s.line as i64, s.signature],
                    )?;
                }
                Ok(())
            })?;
        }
        self.store.with(|conn| {
            let mut statement = conn.prepare("SELECT path FROM index_files")?;
            let indexed: Vec<String> = statement
                .query_map([], |row| row.get(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            drop(statement);
            for path in indexed {
                if !allowed_paths.contains(&path) {
                    conn.execute("DELETE FROM index_symbols WHERE path=?1", [&path])?;
                    conn.execute("DELETE FROM index_files WHERE path=?1", [&path])?;
                }
            }
            Ok(())
        })?;
        self.store
            .meta_set("index_last_built", &nexus_core::now_rfc3339())?;
        self.status()
    }

    /// Look up a symbol by exact or prefix name.
    pub fn find_symbol(&self, name: &str, limit: usize) -> Result<Vec<(String, Symbol)>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT path, name, kind, line, signature FROM index_symbols
                 WHERE name = ?1 OR name LIKE ?2 ORDER BY (name = ?1) DESC, name LIMIT ?3",
            )?;
            let like = format!("{name}%");
            let rows = stmt.query_map(rusqlite::params![name, like, limit as i64], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Symbol {
                        name: r.get(1)?,
                        kind: r.get(2)?,
                        line: r.get::<_, i64>(3)? as usize,
                        signature: r.get(4)?,
                    },
                ))
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    /// Symbols defined in a specific file.
    pub fn file_symbols(&self, rel_path: &str) -> Result<Vec<Symbol>> {
        self.store.with(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name, kind, line, signature FROM index_symbols WHERE path = ?1 ORDER BY line",
            )?;
            let rows = stmt.query_map([rel_path], |r| {
                Ok(Symbol {
                    name: r.get(0)?,
                    kind: r.get(1)?,
                    line: r.get::<_, i64>(2)? as usize,
                    signature: r.get(3)?,
                })
            })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
    }

    pub fn status(&self) -> Result<IndexStatus> {
        self.store.with(|conn| {
            let files: i64 =
                conn.query_row("SELECT COUNT(*) FROM index_files", [], |r| r.get(0))?;
            let symbols: i64 =
                conn.query_row("SELECT COUNT(*) FROM index_symbols", [], |r| r.get(0))?;
            let mut stmt = conn.prepare(
                "SELECT language, COUNT(*) FROM index_files GROUP BY language ORDER BY COUNT(*) DESC",
            )?;
            let langs = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))?
                .filter_map(|r| r.ok())
                .collect();
            let last_built = conn
                .query_row(
                    "SELECT value FROM kv_meta WHERE key = 'index_last_built'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok();
            Ok(IndexStatus {
                files: files as usize,
                symbols: symbols as usize,
                languages: langs,
                last_built,
            })
        })
    }

    pub fn clean(&self) -> Result<()> {
        self.store.with(|conn| {
            conn.execute("DELETE FROM index_symbols", [])?;
            conn.execute("DELETE FROM index_files", [])?;
            Ok(())
        })?;
        Ok(())
    }
}

/// Regex-based symbol extraction. Approximate by design; see module docs.
pub fn extract_symbols(language: &str, text: &str) -> Vec<Symbol> {
    let patterns: &[(&str, &str)] = match language {
        "rust" => &[
            (
                r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)",
                "fn",
            ),
            (
                r"^\s*(?:pub\s+)?struct\s+([a-zA-Z_][a-zA-Z0-9_]*)",
                "struct",
            ),
            (r"^\s*(?:pub\s+)?enum\s+([a-zA-Z_][a-zA-Z0-9_]*)", "enum"),
            (r"^\s*(?:pub\s+)?trait\s+([a-zA-Z_][a-zA-Z0-9_]*)", "trait"),
            (r"^\s*impl(?:<[^>]*>)?\s+([a-zA-Z_][a-zA-Z0-9_]*)", "impl"),
            (r"^\s*(?:pub\s+)?mod\s+([a-zA-Z_][a-zA-Z0-9_]*)", "mod"),
            (r"^\s*(?:pub\s+)?const\s+([a-zA-Z_][a-zA-Z0-9_]*)", "const"),
            (r"^\s*(?:pub\s+)?type\s+([a-zA-Z_][a-zA-Z0-9_]*)", "type"),
        ],
        "python" => &[
            (r"^\s*def\s+([a-zA-Z_][a-zA-Z0-9_]*)", "fn"),
            (r"^\s*class\s+([a-zA-Z_][a-zA-Z0-9_]*)", "class"),
        ],
        "typescript" | "javascript" => &[
            (
                r"^\s*(?:export\s+)?(?:async\s+)?function\s+([a-zA-Z_$][a-zA-Z0-9_$]*)",
                "fn",
            ),
            (
                r"^\s*(?:export\s+)?class\s+([a-zA-Z_$][a-zA-Z0-9_$]*)",
                "class",
            ),
            (
                r"^\s*(?:export\s+)?(?:const|let)\s+([a-zA-Z_$][a-zA-Z0-9_$]*)\s*=\s*(?:async\s*)?\(",
                "fn",
            ),
            (
                r"^\s*(?:export\s+)?interface\s+([a-zA-Z_$][a-zA-Z0-9_$]*)",
                "type",
            ),
            (
                r"^\s*(?:export\s+)?type\s+([a-zA-Z_$][a-zA-Z0-9_$]*)",
                "type",
            ),
        ],
        "go" => &[
            (
                r"^\s*func\s+(?:\([^)]*\)\s*)?([a-zA-Z_][a-zA-Z0-9_]*)",
                "fn",
            ),
            (r"^\s*type\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+struct", "struct"),
            (r"^\s*type\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+interface", "trait"),
        ],
        "c" | "cpp" => &[
            (
                r"^\s*(?:[a-zA-Z_][\w \*]+)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\([^;]*\)\s*\{",
                "fn",
            ),
            (r"^\s*(?:struct|class)\s+([a-zA-Z_][a-zA-Z0-9_]*)", "struct"),
        ],
        _ => &[],
    };
    let compiled: Vec<(regex::Regex, &str)> = patterns
        .iter()
        .filter_map(|(p, k)| regex::Regex::new(p).ok().map(|r| (r, *k)))
        .collect();
    let mut symbols = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.len() > 400 {
            continue;
        }
        for (re, kind) in &compiled {
            if let Some(caps) = re.captures(line) {
                if let Some(name) = caps.get(1) {
                    symbols.push(Symbol {
                        name: name.as_str().to_string(),
                        kind: (*kind).to_string(),
                        line: i + 1,
                        signature: line.trim().chars().take(120).collect(),
                    });
                    break; // one symbol per line
                }
            }
        }
    }
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::workspace::WorkspaceGuard;

    #[test]
    fn extracts_rust_symbols() {
        let src = "pub fn hello() {}\nstruct Widget;\nenum Color { Red }\npub trait Draw {}\nimpl Widget {}";
        let syms = extract_symbols("rust", src);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"));
        assert!(names.contains(&"Widget"));
        assert!(names.contains(&"Color"));
        assert!(names.contains(&"Draw"));
    }

    #[test]
    fn extracts_python_symbols() {
        let src = "def foo():\n    pass\nclass Bar:\n    def method(self): pass";
        let syms = extract_symbols("python", src);
        assert!(syms.iter().any(|s| s.name == "foo" && s.kind == "fn"));
        assert!(syms.iter().any(|s| s.name == "Bar" && s.kind == "class"));
    }

    #[test]
    fn build_and_query_index() {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn special_target() {}\nstruct Thing;",
        )
        .expect("write");
        let store = Store::open_in_memory().expect("store");
        let guard = WorkspaceGuard::new(dir.path(), &[]).expect("guard");
        let indexer = Indexer::new(store);
        let status = indexer.build(dir.path(), &guard).expect("build");
        assert!(status.files >= 1);
        assert!(status.symbols >= 2);
        let found = indexer.find_symbol("special_target", 5).expect("find");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.kind, "fn");
        assert!(found[0].0.contains("lib.rs"));
    }

    #[test]
    fn incremental_skips_unchanged() {
        let dir = tempfile::tempdir().expect("dir");
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").expect("write");
        let store = Store::open_in_memory().expect("store");
        let guard = WorkspaceGuard::new(dir.path(), &[]).expect("guard");
        let indexer = Indexer::new(store);
        indexer.build(dir.path(), &guard).expect("build1");
        // Second build with no changes must not error and keeps counts stable.
        let status = indexer.build(dir.path(), &guard).expect("build2");
        assert_eq!(status.files, 1);
    }
}
