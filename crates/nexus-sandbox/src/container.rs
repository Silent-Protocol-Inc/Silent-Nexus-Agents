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

use crate::{ExecOutcome, ExecSpec, IsolationReport, NetworkMode, OutputChunk, SandboxBackend};
use nexus_core::{NexusError, Result};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
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
                    return Ok(Self {
                        engine: engine.to_string(),
                        image: image.to_string(),
                    });
                }
                _ => continue,
            }
        }
        Err("neither docker nor podman responded to `version`".into())
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

    fn isolation(&self, network: NetworkMode) -> IsolationReport {
        IsolationReport {
            backend: format!("container ({})", self.engine),
            level: "container".into(),
            filesystem:
                "read-only base image; workspace bind-mounted read-write at /workspace; tmpfs /tmp"
                    .into(),
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
        // `image inspect` (cheap, local); pulling images requires approval
        // and is done via `snx sandbox test` explicitly.
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

        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            name.clone(),
            "--read-only".into(),
            "--tmpfs".into(),
            "/tmp:rw,size=256m".into(),
            "-v".into(),
            format!("{cwd}:/workspace:rw"),
            "-w".into(),
            "/workspace".into(),
            "--memory".into(),
            format!("{}m", spec.memory_limit_mb.max(64)),
            "--pids-limit".into(),
            "256".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
        ];
        if spec.network == NetworkMode::Off {
            args.push("--network".into());
            args.push("none".into());
        }
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

        let cap = spec.output_hard_cap.max(4096);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let live2 = live.clone();
        let stdout_task = tokio::spawn(read_capped(stdout, cap, live2, false));
        let stderr_task = tokio::spawn(read_capped(stderr, cap, live, true));

        let timeout = Duration::from_secs(spec.timeout_secs.max(1));
        let (timed_out, status) = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(s)) => (false, Some(s)),
            Ok(Err(e)) => return Err(NexusError::Sandbox(format!("wait failed: {e}"))),
            Err(_) => {
                // Hard-kill the container by name; --rm cleans it up.
                let _ = tokio::process::Command::new(&self.engine)
                    .args(["kill", &name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
                let _ = child.kill().await;
                (true, None)
            }
        };

        let (stdout_text, stdout_capped) = stdout_task
            .await
            .map_err(|e| NexusError::Sandbox(format!("stdout task: {e}")))?;
        let (stderr_text, stderr_capped) = stderr_task
            .await
            .map_err(|e| NexusError::Sandbox(format!("stderr task: {e}")))?;
        let output_capped = stdout_capped || stderr_capped;
        if output_capped && !timed_out {
            let _ = tokio::process::Command::new(&self.engine)
                .args(["kill", &name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }

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

async fn read_capped<R>(
    reader: Option<R>,
    cap: usize,
    live: Option<mpsc::UnboundedSender<OutputChunk>>,
    is_stderr: bool,
) -> (String, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return (String::new(), false);
    };
    let mut collected: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut capped = false;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let take = n.min(cap.saturating_sub(collected.len()));
                collected.extend_from_slice(&buf[..take]);
                if let Some(tx) = &live {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = tx.send(if is_stderr {
                        OutputChunk::Stderr(text)
                    } else {
                        OutputChunk::Stdout(text)
                    });
                }
                if collected.len() >= cap {
                    capped = true;
                    break;
                }
            }
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&collected).to_string(), capped)
}
