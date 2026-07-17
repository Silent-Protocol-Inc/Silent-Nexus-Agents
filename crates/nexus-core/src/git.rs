//! Centralized, bounded Git subprocess execution.

use crate::{NexusError, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_OUTPUT_CAP: usize = 1_048_576;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub output_capped: bool,
}

#[derive(Debug, Clone)]
pub struct GitRunner {
    workspace: PathBuf,
    allow_hooks: bool,
    output_cap: usize,
    timeout: Duration,
}

impl GitRunner {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            allow_hooks: false,
            output_cap: DEFAULT_OUTPUT_CAP,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Hooks are disabled by default. Callers may enable them only after an
    /// explicit, typed operator confirmation.
    pub fn with_hooks(mut self, allow_hooks: bool) -> Self {
        self.allow_hooks = allow_hooks;
        self
    }

    pub fn with_output_cap(mut self, bytes: usize) -> Self {
        self.output_cap = bytes.clamp(4_096, 8 * 1024 * 1024);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn run(&self, args: &[&str]) -> Result<GitOutput> {
        let owned = args.iter().map(OsString::from).collect::<Vec<_>>();
        self.run_os(&owned)
    }

    pub fn run_owned(&self, args: &[String]) -> Result<GitOutput> {
        let owned = args.iter().map(OsString::from).collect::<Vec<_>>();
        self.run_os(&owned)
    }

    pub fn checked(&self, label: &str, args: &[&str]) -> Result<String> {
        let output = self.run(args)?;
        if output.success {
            Ok(output.stdout)
        } else {
            let detail = if output.stderr.trim().is_empty() {
                "Git returned a non-zero status".to_string()
            } else {
                output.stderr.trim().to_string()
            };
            Err(NexusError::Other(format!("{label} failed: {detail}")))
        }
    }

    fn run_os(&self, args: &[OsString]) -> Result<GitOutput> {
        let mut command = std::process::Command::new("git");
        command
            .current_dir(&self.workspace)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        copy_safe_environment(&mut command);
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "Never")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .arg("-c")
            .arg("core.pager=cat")
            .arg("-c")
            .arg("pager.branch=false")
            .arg("-c")
            .arg("pager.diff=false")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-c")
            .arg("commit.gpgSign=false")
            .arg("-c")
            .arg("tag.gpgSign=false");
        if !self.allow_hooks {
            command.arg("-c").arg("core.hooksPath=/dev/null");
        }
        append_safe_args(&mut command, args);

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            #[allow(unsafe_code)]
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let mut child = command
            .spawn()
            .map_err(|error| NexusError::Other(format!("launching Git: {error}")))?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| NexusError::Other("Git stdout was not captured".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| NexusError::Other("Git stderr was not captured".into()))?;

        let total = Arc::new(AtomicUsize::new(0));
        let capped = Arc::new(AtomicBool::new(false));
        let stdout_task = spawn_reader(stdout, self.output_cap, total.clone(), capped.clone());
        let stderr_task = spawn_reader(stderr, self.output_cap, total, capped.clone());

        let started = Instant::now();
        let status: ExitStatus = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if capped.load(Ordering::Acquire) || started.elapsed() >= self.timeout {
                kill_process_group(pid);
                let _ = child.kill();
                break child.wait()?;
            }
            std::thread::sleep(Duration::from_millis(5));
        };

        let stdout = stdout_task
            .join()
            .map_err(|_| NexusError::Other("Git stdout reader panicked".into()))??;
        let stderr = stderr_task
            .join()
            .map_err(|_| NexusError::Other("Git stderr reader panicked".into()))??;
        let redactor = crate::redact::Redactor::new();
        Ok(GitOutput {
            success: status.success() && !capped.load(Ordering::Acquire),
            code: status.code(),
            stdout: crate::sanitize::sanitize_terminal(
                redactor
                    .redact(&String::from_utf8_lossy(&stdout))
                    .trim_end(),
            ),
            stderr: crate::sanitize::sanitize_terminal(
                redactor
                    .redact(&String::from_utf8_lossy(&stderr))
                    .trim_end(),
            ),
            output_capped: capped.load(Ordering::Acquire),
        })
    }
}

fn append_safe_args(command: &mut std::process::Command, args: &[OsString]) {
    let diff_subcommand = args.first().is_some_and(|argument| {
        matches!(
            argument.to_str(),
            Some("diff" | "show" | "log" | "format-patch")
        )
    });

    for (index, argument) in args.iter().enumerate() {
        command.arg(argument);
        if diff_subcommand && index == 0 {
            command.arg("--no-ext-diff");
            command.arg("--no-textconv");
        }
    }
}

fn copy_safe_environment(command: &mut std::process::Command) {
    for key in [
        "PATH", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "USER",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn spawn_reader<R>(
    mut reader: R,
    cap: usize,
    total: Arc<AtomicUsize>,
    capped: Arc<AtomicBool>,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 8_192];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let mut current = total.load(Ordering::Acquire);
            loop {
                if current >= cap {
                    capped.store(true, Ordering::Release);
                    return Ok(output);
                }
                let take = count.min(cap - current);
                match total.compare_exchange(
                    current,
                    current + take,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        output.extend_from_slice(&buffer[..take]);
                        if take < count || current + take >= cap {
                            capped.store(true, Ordering::Release);
                        }
                        break;
                    }
                    Err(actual) => current = actual,
                }
            }
            if capped.load(Ordering::Acquire) {
                break;
            }
        }
        Ok(output)
    })
}

fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        #[allow(unsafe_code)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("directory");
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(directory.path())
                .env("GIT_AUTHOR_NAME", "NEXUS Test")
                .env("GIT_AUTHOR_EMAIL", "nexus@example.invalid")
                .env("GIT_COMMITTER_NAME", "NEXUS Test")
                .env("GIT_COMMITTER_EMAIL", "nexus@example.invalid")
                .status()
                .expect("git");
            assert!(status.success());
        };
        run(&["init", "-q", "--initial-branch=main"]);
        run(&["config", "user.name", "NEXUS Test"]);
        run(&["config", "user.email", "nexus@example.invalid"]);
        std::fs::write(directory.path().join("a.txt"), "one\n").expect("write");
        run(&["add", "a.txt"]);
        run(&["commit", "-qm", "initial"]);
        directory
    }

    #[test]
    fn dangerous_git_environment_is_removed() {
        let directory = repository();
        // Parallel tests spawn git processes that inherit this process env.
        // GIT_CONFIG_COUNT must never be visible without its KEY/VALUE pair,
        // or those spawns abort with "missing config key GIT_CONFIG_KEY_0".
        std::env::set_var("GIT_CONFIG_KEY_0", "alias.status");
        std::env::set_var("GIT_CONFIG_VALUE_0", "!exit 91");
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        let output = GitRunner::new(directory.path())
            .run(&["status", "--short"])
            .expect("run");
        std::env::remove_var("GIT_CONFIG_COUNT");
        std::env::remove_var("GIT_CONFIG_KEY_0");
        std::env::remove_var("GIT_CONFIG_VALUE_0");
        assert!(output.success, "{output:?}");
    }

    #[cfg(unix)]
    #[test]
    fn hooks_are_disabled_unless_explicitly_enabled() {
        use std::os::unix::fs::PermissionsExt;
        let directory = repository();
        let hook = directory.path().join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\ntouch hook-ran\n").expect("hook");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("mode");
        std::fs::write(directory.path().join("a.txt"), "two\n").expect("write");
        let runner = GitRunner::new(directory.path());
        assert!(runner.run(&["add", "a.txt"]).expect("add").success);
        assert!(
            runner
                .run(&["commit", "-m", "without hook"])
                .expect("commit")
                .success
        );
        assert!(!directory.path().join("hook-ran").exists());

        std::fs::write(directory.path().join("a.txt"), "three\n").expect("write");
        assert!(runner.run(&["add", "a.txt"]).expect("add").success);
        assert!(
            runner
                .clone()
                .with_hooks(true)
                .run(&["commit", "-m", "with hook"])
                .expect("commit")
                .success
        );
        assert!(directory.path().join("hook-ran").exists());
    }

    #[cfg(unix)]
    #[test]
    fn repository_external_diff_configuration_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let directory = repository();
        let external = directory.path().join("external-diff");
        std::fs::write(&external, "#!/bin/sh\ntouch external-diff-ran\nexit 99\n").expect("script");
        std::fs::set_permissions(&external, std::fs::Permissions::from_mode(0o755)).expect("mode");
        let status = std::process::Command::new("git")
            .args(["config", "diff.external", external.to_str().expect("path")])
            .current_dir(directory.path())
            .status()
            .expect("git config");
        assert!(status.success());
        std::fs::write(directory.path().join("a.txt"), "changed\n").expect("write");

        let output = GitRunner::new(directory.path())
            .run(&["diff"])
            .expect("safe diff");
        assert!(output.success, "{output:?}");
        assert!(output.stdout.contains("changed"));
        assert!(!directory.path().join("external-diff-ran").exists());
    }

    #[cfg(unix)]
    #[test]
    fn repository_textconv_configuration_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let directory = repository();
        let textconv = directory.path().join("textconv");
        std::fs::write(&textconv, "#!/bin/sh\ntouch textconv-ran\ncat \"$1\"\n").expect("script");
        std::fs::set_permissions(&textconv, std::fs::Permissions::from_mode(0o755)).expect("mode");
        std::fs::write(directory.path().join(".gitattributes"), "a.txt diff=demo\n")
            .expect("attributes");
        let runner = GitRunner::new(directory.path());
        assert!(
            runner
                .run_owned(&[
                    "config".into(),
                    "diff.demo.textconv".into(),
                    textconv.display().to_string(),
                ])
                .expect("config")
                .success
        );
        std::fs::write(directory.path().join("a.txt"), "changed\n").expect("write");

        let output = runner.run(&["diff"]).expect("safe diff");
        assert!(output.success, "{output:?}");
        assert!(!directory.path().join("textconv-ran").exists());
    }
}
