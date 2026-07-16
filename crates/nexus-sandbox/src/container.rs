//! Container backend using Docker or Podman.
//!
//! Isolation provided (and honestly reported):
//! * read-only base filesystem (`--read-only`);
//! * workspace bind-mounted read-write at `/workspace`;
//! * writable tmpfs at `/tmp`;
//! * `--network none` unless network was approved;
//! * memory / CPU / pids limits;
//! * container removed after each run (`--rm`), named for hard kill on
//!   timeout.

use crate::{
    ExecOutcome, ExecSpec, FilesystemAccess, IsolationReport, IsolationStrength, NetworkMode,
    OutputChunk, SandboxBackend,
};
use nexus_core::{NexusError, Result};
use std::path::{Component, Path};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ContainerBackend {
    engine: String, // "docker" or "podman"
    image: String,
}

impl ContainerBackend {
    /// Detect a working container engine. Errors with the concrete reason
    /// when neither Docker nor Podman responds.
    pub async fn detect(image: &str) -> std::result::Result<Self, String> {
        let mut reasons = Vec::new();
        for engine in ["docker", "podman"] {
            let probe = tokio::process::Command::new(engine)
                .arg("version")
                .arg("--format")
                .arg("{{.Client.Version}}")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output();
            match tokio::time::timeout(Duration::from_secs(5), probe).await {
                Ok(Ok(out)) if out.status.success() => {
                    let backend = Self {
                        engine: engine.to_string(),
                        image: image.to_string(),
                    };
                    match backend.availability().await {
                        Ok(_) => return Ok(backend),
                        Err(reason) => reasons.push(reason),
                    }
                }
                _ => reasons.push(format!("{engine} did not respond to `version`")),
            }
        }
        Err(reasons.join("; "))
    }

    pub fn engine(&self) -> &str {
        &self.engine
    }
}

#[async_trait::async_trait]
impl SandboxBackend for ContainerBackend {
    fn name(&self) -> &'static str {
        "container"
    }

    fn strength(&self) -> IsolationStrength {
        IsolationStrength::Strong
    }

    fn isolation(&self, network: NetworkMode) -> IsolationReport {
        IsolationReport {
            backend: format!("container ({})", self.engine),
            strength: IsolationStrength::Strong,
            level: "container".into(),
            filesystem:
                "read-only base image; workspace mounted read-only or writable per approved action; private paths masked; tmpfs /tmp"
                    .into(),
            filesystem_access: FilesystemAccess::WorkspaceWrite,
            network: match network {
                NetworkMode::Off => "disabled (--network none)".into(),
                NetworkMode::Restricted => {
                    "container network up; destination filtering at tool layer".into()
                }
                NetworkMode::Full => "container default network".into(),
            },
            resources:
                "memory, cpu and pids limits via engine cgroups; wall-clock timeout with hard kill"
                    .into(),
            caveats: vec![
                "shares the host kernel; not a VM".into(),
                "workspace mount is writable by design".into(),
            ],
        }
    }

    async fn availability(&self) -> std::result::Result<String, String> {
        // `image inspect` is cheap and local. Image pulls remain an explicit
        // operator action outside Silent Nexus.
        let out = tokio::process::Command::new(&self.engine)
            .args(["image", "inspect", &self.image])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| format!("{} not runnable: {e}", self.engine))?;
        if out.success() {
            Ok(format!("{} ready with image {}", self.engine, self.image))
        } else {
            Err(format!(
                "image `{}` not present locally; run `{} pull {}` (snx does not pull images without approval)",
                self.image, self.engine, self.image
            ))
        }
    }

    async fn execute(
        &self,
        spec: ExecSpec,
        live: Option<mpsc::UnboundedSender<OutputChunk>>,
    ) -> Result<ExecOutcome> {
        let start = Instant::now();
        let name = format!("snx-{}", uuid::Uuid::new_v4().simple());
        let cwd = spec
            .cwd
            .to_str()
            .ok_or_else(|| NexusError::Sandbox("non-UTF8 workspace path".into()))?;

        let effective_network = spec.effective_network();
        #[cfg(unix)]
        #[allow(unsafe_code)]
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        #[cfg(not(unix))]
        let (uid, gid) = (65_534u32, 65_534u32);

        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            name.clone(),
            "--log-driver".into(),
            "none".into(),
            "--read-only".into(),
            "--tmpfs".into(),
            format!("/tmp:rw,nosuid,nodev,size=256m,uid={uid},gid={gid}"),
            "--user".into(),
            format!("{uid}:{gid}"),
            "--hostname".into(),
            "snx".into(),
            "--ipc".into(),
            "none".into(),
            "--memory".into(),
            format!("{}m", spec.memory_limit_mb.max(64)),
            "--pids-limit".into(),
            "256".into(),
            "--cpus".into(),
            "1.0".into(),
            "--ulimit".into(),
            format!(
                "cpu={}:{}",
                spec.cpu_limit_secs.max(1),
                spec.cpu_limit_secs.max(1)
            ),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
        ];
        match spec.filesystem_access {
            FilesystemAccess::NoWorkspace => {
                args.push("-w".into());
                args.push("/tmp".into());
            }
            FilesystemAccess::ReadOnly | FilesystemAccess::WorkspaceWrite => {
                let mode = if spec.filesystem_access == FilesystemAccess::ReadOnly {
                    "ro"
                } else {
                    "rw"
                };
                args.push("-v".into());
                args.push(format!("{cwd}:/workspace:{mode}"));
                args.push("-w".into());
                args.push("/workspace".into());
                add_sensitive_masks(&mut args, &spec, Path::new(cwd))?;
            }
        }
        if effective_network == NetworkMode::Off {
            args.push("--network".into());
            args.push("none".into());
        }
        args.push("-e".into());
        args.push("HOME=/tmp".into());
        for (k, v) in crate::scrubbed_env(&spec.env_allowlist, &spec.env) {
            // PATH/HOME from the host are meaningless inside the container.
            if k == "PATH" || k == "HOME" {
                continue;
            }
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }
        if spec.stdin.is_some() {
            args.push("-i".into());
        }
        args.push(self.image.clone());
        if spec.shell {
            args.push("/bin/sh".into());
            args.push("-c".into());
            args.push(spec.program.clone());
        } else {
            args.push(spec.program.clone());
            args.extend(spec.args.iter().cloned());
        }

        let mut child = tokio::process::Command::new(&self.engine)
            .args(&args)
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| NexusError::Sandbox(format!("failed to run {}: {e}", self.engine)))?;

        if let (Some(data), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(data.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let budget = crate::output::OutputBudget::new(spec.output_hard_cap);
        let live2 = live.clone();
        let stdout_task = tokio::spawn(crate::output::read_stream(
            stdout,
            budget.clone(),
            live2,
            false,
        ));
        let stderr_task = tokio::spawn(crate::output::read_stream(
            stderr,
            budget.clone(),
            live,
            true,
        ));

        let timeout = Duration::from_secs(spec.timeout_secs.max(1));
        enum Completion {
            Status(std::process::ExitStatus),
            Timeout,
            OutputCap,
        }
        let completion = {
            let wait = child.wait();
            tokio::pin!(wait);
            tokio::select! {
                status = &mut wait => Completion::Status(
                    status.map_err(|error| NexusError::Sandbox(format!("wait failed: {error}")))?
                ),
                _ = tokio::time::sleep(timeout) => Completion::Timeout,
                _ = budget.wait_capped() => Completion::OutputCap,
            }
        };
        let (timed_out, status) = match completion {
            Completion::Status(status) => (false, Some(status)),
            Completion::Timeout => {
                terminate_container(&self.engine, &name, &mut child).await;
                (true, None)
            }
            Completion::OutputCap => {
                terminate_container(&self.engine, &name, &mut child).await;
                (false, None)
            }
        };

        let stdout_text = stdout_task
            .await
            .map_err(|e| NexusError::Sandbox(format!("stdout task: {e}")))?;
        let stderr_text = stderr_task
            .await
            .map_err(|e| NexusError::Sandbox(format!("stderr task: {e}")))?;
        let output_capped = budget.is_capped();

        Ok(ExecOutcome {
            exit_code: status.and_then(|s| s.code()),
            stdout: stdout_text,
            stderr: stderr_text,
            duration_ms: start.elapsed().as_millis() as u64,
            timed_out,
            output_capped,
            backend: format!("container ({})", self.engine),
            isolation: "container".into(),
        })
    }
}

async fn terminate_container(engine: &str, name: &str, child: &mut tokio::process::Child) {
    // Close the attached client immediately so its stdout/stderr pipes cannot
    // delay a cap or timeout response while the daemon kills the container.
    let _ = child.start_kill();
    let (_, _) = tokio::join!(kill_container(engine, name), child.wait());
}

async fn kill_container(engine: &str, name: &str) {
    let _ = tokio::process::Command::new(engine)
        .args(["kill", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

fn add_sensitive_masks(args: &mut Vec<String>, spec: &ExecSpec, workspace: &Path) -> Result<()> {
    for relative in &spec.sensitive_path_masks {
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(NexusError::PathDenied(format!(
                "invalid sensitive-path mask {}",
                relative.display()
            )));
        }
        let source = workspace.join(relative);
        let Ok(metadata) = std::fs::symlink_metadata(&source) else {
            continue;
        };
        let target = format!("/workspace/{}", relative.display());
        args.push("--mount".into());
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            args.push(format!(
                "type=tmpfs,destination={target},tmpfs-mode=000,tmpfs-size=65536"
            ));
        } else {
            args.push(format!(
                "type=bind,source=/dev/null,destination={target},readonly"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn spec(workspace: &Path, masks: Vec<PathBuf>) -> ExecSpec {
        ExecSpec {
            program: "true".into(),
            args: Vec::new(),
            shell: false,
            cwd: workspace.to_path_buf(),
            env: BTreeMap::new(),
            env_allowlist: Vec::new(),
            network: NetworkMode::Off,
            approved_network: NetworkMode::Off,
            filesystem_access: FilesystemAccess::ReadOnly,
            sensitive_path_masks: masks,
            unsafe_host_approved: false,
            timeout_secs: 5,
            cpu_limit_secs: 5,
            memory_limit_mb: 64,
            output_hard_cap: 4_096,
            stdin: None,
        }
    }

    #[test]
    fn sensitive_directories_and_files_are_replaced_by_inaccessible_mounts() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join(".git")).expect("git");
        std::fs::write(workspace.path().join(".env"), "TOKEN=secret").expect("env");
        let spec = spec(
            workspace.path(),
            vec![PathBuf::from(".git"), PathBuf::from(".env")],
        );
        let mut arguments = Vec::new();
        add_sensitive_masks(&mut arguments, &spec, workspace.path()).expect("masks");
        assert!(arguments.iter().any(|argument| {
            argument.contains("type=tmpfs,destination=/workspace/.git")
                && argument.contains("tmpfs-mode=000")
        }));
        assert!(arguments.iter().any(|argument| {
            argument == "type=bind,source=/dev/null,destination=/workspace/.env,readonly"
        }));
    }

    #[test]
    fn sensitive_mask_paths_cannot_escape_the_workspace() {
        let workspace = tempfile::tempdir().expect("workspace");
        let spec = spec(workspace.path(), vec![PathBuf::from("../outside")]);
        assert!(add_sensitive_masks(&mut Vec::new(), &spec, workspace.path()).is_err());
    }
}
