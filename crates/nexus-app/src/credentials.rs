//! Unified secure credential storage for provider API keys and tokens.
//!
//! Layout: `<config>/auth/<provider>/<profile>.cred`, directory mode `0700`,
//! files `0600`. Each file is JSON `{version, created_at, secret}`; the secret
//! is wrapped in [`SecretString`] the moment it is read so Debug/serialize
//! paths print `[redacted]`.
//!
//! # Honest storage limitations
//!
//! OS keyrings (Secret Service / Keychain / Credential Manager) are the
//! stronger home for secrets, but they need a running keyring daemon and a
//! native dependency; on headless Linux (this product's primary target) none
//! is reliably present. Silent Nexus therefore uses restricted-permission
//! files and is explicit about it: any process running as your user can read
//! them, the same trust level as `~/.codex/auth.json` or `~/.aws/credentials`.
//! The storage backend is reported by `snx auth status` and `/login`.

use nexus_core::{NexusError, Result, SecretString};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CRED_VERSION: u32 = 1;

/// Metadata about a stored credential (never the value).
#[derive(Debug, Clone, Serialize)]
pub struct CredentialInfo {
    pub provider: String,
    pub profile: String,
    pub created_at: String,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct CredFile {
    version: u32,
    created_at: String,
    secret: String,
}

/// File-backed credential store rooted at the auth dir.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    root: PathBuf,
}

impl CredentialStore {
    pub fn new(auth_dir: &Path) -> Self {
        Self {
            root: auth_dir.to_path_buf(),
        }
    }

    /// Human description of the backend, shown in auth status output.
    pub fn backend_description(&self) -> String {
        format!(
            "restricted files under {} (0700 dir / 0600 files; no OS keyring on this build — \
             readable by processes running as your user)",
            self.root.display()
        )
    }

    fn profile_path(&self, provider: &str, profile: &str) -> Result<PathBuf> {
        validate_name(provider)?;
        validate_name(profile)?;
        Ok(self.root.join(provider).join(format!("{profile}.cred")))
    }

    /// Store a secret for `provider`/`profile`, overwriting any existing one.
    pub fn set(&self, provider: &str, profile: &str, secret: &SecretString) -> Result<()> {
        if secret.is_empty() {
            return Err(NexusError::Config(
                "refusing to store an empty secret".into(),
            ));
        }
        let path = self.profile_path(provider, profile)?;
        let dir = path.parent().expect("profile path has a parent");
        std::fs::create_dir_all(dir)?;
        restrict_dir(dir)?;
        restrict_dir(&self.root)?;
        let body = serde_json::to_string(&CredFile {
            version: CRED_VERSION,
            created_at: nexus_core::now_rfc3339(),
            secret: secret.expose().to_string(),
        })
        .map_err(|e| NexusError::Other(format!("serializing credential: {e}")))?;
        write_restricted(&path, &body)?;
        Ok(())
    }

    /// Load a secret. `Ok(None)` when the profile does not exist.
    pub fn get(&self, provider: &str, profile: &str) -> Result<Option<SecretString>> {
        let path = self.profile_path(provider, profile)?;
        if !path.exists() {
            return Ok(None);
        }
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(NexusError::PathDenied(format!(
                "credential path is a symlink: {}",
                path.display()
            )));
        }
        nexus_core::permissions::repair_private_tree(&self.root)?;
        let text = std::fs::read_to_string(&path)?;
        let parsed: CredFile = serde_json::from_str(&text).map_err(|e| {
            NexusError::Config(format!(
                "{} is not a valid credential file: {e}",
                path.display()
            ))
        })?;
        if parsed.secret.is_empty() {
            return Ok(None);
        }
        Ok(Some(SecretString::new(parsed.secret)))
    }

    /// Resolve an `api_key_ref` of the form `provider/profile` (or just
    /// `profile`, meaning provider `custom`).
    pub fn resolve_ref(&self, key_ref: &str) -> Result<Option<SecretString>> {
        let (provider, profile) = match key_ref.split_once('/') {
            Some((p, n)) => (p, n),
            None => ("custom", key_ref),
        };
        self.get(provider, profile)
    }

    /// Remove a credential. `Ok(false)` when it did not exist.
    pub fn remove(&self, provider: &str, profile: &str) -> Result<bool> {
        let path = self.profile_path(provider, profile)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path)?;
        Ok(true)
    }

    /// Enumerate stored credentials (metadata only).
    pub fn list(&self) -> Result<Vec<CredentialInfo>> {
        let mut out = Vec::new();
        let Ok(providers) = std::fs::read_dir(&self.root) else {
            return Ok(out);
        };
        for provider_entry in providers.flatten() {
            let provider_path = provider_entry.path();
            if !provider_path.is_dir() {
                continue;
            }
            let provider = provider_entry.file_name().to_string_lossy().to_string();
            let Ok(profiles) = std::fs::read_dir(&provider_path) else {
                continue;
            };
            for profile_entry in profiles.flatten() {
                let path = profile_entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("cred") {
                    continue;
                }
                let profile = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let created_at = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str::<CredFile>(&t).ok())
                    .map(|c| c.created_at)
                    .unwrap_or_else(|| "unknown".into());
                out.push(CredentialInfo {
                    provider: provider.clone(),
                    profile,
                    created_at,
                    path,
                });
            }
        }
        out.sort_by(|a, b| (&a.provider, &a.profile).cmp(&(&b.provider, &b.profile)));
        Ok(out)
    }

    /// Whether a credential exists without reading it.
    pub fn exists(&self, provider: &str, profile: &str) -> bool {
        self.profile_path(provider, profile)
            .map(|p| p.exists())
            .unwrap_or(false)
    }
}

fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        return Err(NexusError::Config(format!(
            "invalid credential name `{name}`: use letters, digits, `-`, `_`, `.`"
        )));
    }
    // A name that is only dots could escape the directory.
    if name.chars().all(|c| c == '.') {
        return Err(NexusError::Config("invalid credential name".into()));
    }
    Ok(())
}

fn write_restricted(path: &Path, body: &str) -> Result<()> {
    nexus_core::atomic::atomic_write_private(path, body.as_bytes())
}

fn restrict_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, CredentialStore) {
        let dir = tempfile::tempdir().expect("dir");
        let store = CredentialStore::new(&dir.path().join("auth"));
        (dir, store)
    }

    #[test]
    fn set_get_remove_roundtrip() {
        let (_d, s) = store();
        let secret = SecretString::new("sk-test-123");
        s.set("openai", "default", &secret).expect("set");
        let loaded = s.get("openai", "default").expect("get").expect("some");
        assert_eq!(loaded.expose(), "sk-test-123");
        assert!(s.exists("openai", "default"));
        assert!(s.remove("openai", "default").expect("remove"));
        assert!(s.get("openai", "default").expect("get").is_none());
        assert!(!s.remove("openai", "default").expect("second remove"));
    }

    #[test]
    fn list_reports_metadata_not_secrets() {
        let (_d, s) = store();
        s.set("openai", "work", &SecretString::new("sk-a"))
            .expect("set");
        s.set("openrouter", "main", &SecretString::new("sk-b"))
            .expect("set");
        let list = s.list().expect("list");
        assert_eq!(list.len(), 2);
        let dump = serde_json::to_string(&list).expect("json");
        assert!(!dump.contains("sk-a") && !dump.contains("sk-b"));
    }

    #[test]
    fn rejects_traversal_names() {
        let (_d, s) = store();
        let secret = SecretString::new("x");
        assert!(s.set("../evil", "p", &secret).is_err());
        assert!(s.set("openai", "..", &secret).is_err());
        assert!(s.set("", "p", &secret).is_err());
    }

    #[test]
    fn resolve_ref_forms() {
        let (_d, s) = store();
        s.set("custom", "myapi", &SecretString::new("k1"))
            .expect("set");
        s.set("openrouter", "main", &SecretString::new("k2"))
            .expect("set");
        assert_eq!(
            s.resolve_ref("myapi").expect("ok").expect("some").expose(),
            "k1"
        );
        assert_eq!(
            s.resolve_ref("openrouter/main")
                .expect("ok")
                .expect("some")
                .expose(),
            "k2"
        );
        assert!(s.resolve_ref("missing").expect("ok").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn files_are_restricted() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store();
        s.set("openai", "default", &SecretString::new("sk"))
            .expect("set");
        let info = &s.list().expect("list")[0];
        let mode = std::fs::metadata(&info.path)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
