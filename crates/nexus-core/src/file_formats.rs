//! Stable file-format classification shared by policy, tools, indexing, and UI.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedFormat {
    pub id: &'static str,
    pub hard_denied: bool,
}

pub fn classify(path: &Path) -> ClassifiedFormat {
    let lower_components: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let name = lower_components.last().map(String::as_str).unwrap_or("");
    let hard_denied = lower_components
        .iter()
        .any(|part| matches!(part.as_str(), ".git" | ".nexus"))
        || name == ".env"
        || name.starts_with(".env.")
        || matches!(
            name,
            "credentials" | "credentials.json" | "id_rsa" | "id_ed25519" | "id_ecdsa"
        )
        || matches!(
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "pem" | "key" | "p12" | "pfx" | "keystore"
        );
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let id = match extension.as_str() {
        "rs" => "rust",
        "toml" => "toml",
        "slnt" => "silent",
        "py" | "pyi" => "python",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "go" => "go",
        "java" | "kt" | "kts" => "jvm",
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" => "c_cpp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" | "zsh" | "fish" | "ps1" => "shell",
        "md" | "mdx" | "rst" | "adoc" => "markup",
        "html" | "htm" | "css" | "scss" | "sass" | "less" => "web",
        "json" | "jsonl" | "ndjson" => "json",
        "yaml" | "yml" => "yaml",
        "xml" => "xml",
        "ini" | "cfg" | "conf" | "properties" => "configuration",
        "csv" | "tsv" | "parquet" | "avro" => "data",
        "sql" => "sql",
        "txt" | "log" => "text",
        "pdf" | "doc" | "docx" | "odt" | "rtf" => "document",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => "image",
        "mp3" | "wav" | "flac" | "ogg" | "mp4" | "webm" | "mov" => "media",
        "zip" | "gz" | "bz2" | "xz" | "tar" | "7z" | "rar" => "archive",
        "exe" | "dll" | "so" | "dylib" | "a" | "o" | "wasm" => "binary",
        _ if matches!(
            name,
            "dockerfile"
                | "makefile"
                | "justfile"
                | "cargo.lock"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
        ) =>
        {
            "special"
        }
        _ => "other",
    };
    ClassifiedFormat { id, hard_denied }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_and_sensitive_names() {
        assert_eq!(classify(Path::new("src/lib.rs")).id, "rust");
        assert_eq!(classify(Path::new("sample.slnt")).id, "silent");
        assert!(classify(Path::new(".env.example")).hard_denied);
        assert!(classify(Path::new(".git/config")).hard_denied);
        assert!(classify(Path::new("keys/server.pem")).hard_denied);
        assert!(!classify(Path::new("config/example.toml")).hard_denied);
    }
}
