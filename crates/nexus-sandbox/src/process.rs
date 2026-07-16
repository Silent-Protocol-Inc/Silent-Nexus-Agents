//! Restricted local-process backend.
//!
//! Real controls applied (all verifiable in tests):
//! * scrubbed, allowlisted environment — no inherited secrets;
//! * `setsid` process group so the whole process tree can be killed;
//! * `setrlimit` for CPU seconds, address space, and process count;
//! * wall-clock timeout with process-group SIGKILL;
//! * output hard cap with process termination;
//! * network cut via `unshare(CLONE_NEWUSER|CLONE_NEWNET)` when requested and
//!   permitted by the kernel — when the kernel refuses, the isolation report
//!   says so instead of pretending.
//!
//! This backend does **not** provide a read-only filesystem view; that is the
//! container backend's job. Filesystem protection here comes from the
//! workspace guard at the tool layer plus OS permissions.

use crate::{ExecOutcome, ExecSpec, IsolationReport, NetworkMode, OutputChunk, SandboxBackend};
use nexus_core::{NexusError, Result};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

pub struct ProcessBackend {
    /// True when the user explicitly disabled restrictions (`backend = "none"`).
    unrestricted: bool,
    /// Whether the last availability probe found namespace support.
    netns_supported: AtomicBool,
}

impl ProcessBackend {
    pub fn new(unrestricted: bool) -> Self {
        Self {
            unrestricted,
            netns_supported: AtomicBool::new(probe_netns_support()),
        }
    }
}

/// Probe whether unprivileged user+net namespaces work on this kernel by
/// actually trying them with a trivial command.
fn probe_netns_support() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new("/bin/true");
        // SAFETY: pre_exec runs in the forked child before exec; unshare and
        // the libc calls used are async-signal-safe.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        matches!(cmd.status(), Ok(s) if s.success())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[async_trait::async_trait]
impl SandboxBackend for ProcessBackend {
    fn name(&self) -> &'static str {
        if self.unrestricted {
            "process-unrestricted"
        } else {
            "process-restricted"
        }
    }

    fn isolation(&self, network: NetworkMode) -> IsolationReport {
        if self.unrestricted {
            return IsolationReport {
                backend: self.name().into(),
                level: "path-validation-only".into(),
                filesystem: "no isolation; workspace-boundary checks at tool layer only".into(),
                network: "unrestricted".into(),
                resources: "timeout only".into(),
                caveats: vec![
                    "sandbox disabled in configuration; commands can read anything your user can"
                        .into(),
                ],
            };
        }
        let netns = self.netns_supported.load(Ordering::Relaxed);
        let network_desc = match (network, netns) {
            (NetworkMode::Off, true) => {
                "disabled via user+net namespace (no interfaces)".to_string()
            }
            (NetworkMode::Off, false) => {
                "REQUESTED OFF but kernel denies unprivileged namespaces; network is NOT blocked"
                    .to_string()
            }
            (NetworkMode::Restricted, _) => {
                "OS-level open; destination filtering enforced at tool layer".to_string()
            }
            (NetworkMode::Full, _) => "unrestricted".to_string(),
        };
        IsolationReport {
            backend: self.name().into(),
            level: "process-restricted".into(),
            filesystem: "host filesystem visible; writes confined by workspace guard and OS permissions (not a read-only view)".into(),
            network: network_desc,
            resources: "rlimits: CPU seconds, address space, process count; wall-clock timeout; process-group kill".into(),
            caveats: vec![
                "not a container: kernel attack surface and host paths remain visible".into(),
            ],
        }
    }

    async fn availability(&self) -> std::result::Result<String, String> {
        let netns = probe_netns_support();
        self.netns_supported.store(netns, Ordering::Relaxed);
        Ok(format!(
            "restricted process backend ready (network namespaces: {})",
            if netns { "supported" } else { "unavailable" }
        ))
    }

    async fn execute(
        &self,
        spec: ExecSpec,
        live: Option<mpsc::UnboundedSender<OutputChunk>>,
    ) -> Result<ExecOutcome> {
        let start = Instant::now();
        let mut command = if spec.shell {
            let mut c = tokio::process::Command::new("/bin/sh");
            c.arg("-c").arg(&spec.program);
            c
        } else {
            let mut c = tokio::process::Command::new(&spec.program);
            c.args(&spec.args);
            c
        };
        command
            .current_dir(&spec.cwd)
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(crate::scrubbed_env(&spec.env_allowlist, &spec.env))
            .kill_on_drop(true);

        let restricted = !self.unrestricted;
        let want_netns = restricted
            && spec.network == NetworkMode::Off
            && self.netns_supported.load(Ordering::Relaxed);
        let cpu = spec.cpu_limit_secs;
        let mem_bytes = spec.memory_limit_mb.saturating_mul(1024 * 1024);
        #[cfg(unix)]
        {
            // SAFETY: only async-signal-safe libc calls in the forked child.
            #[allow(unsafe_code)]
            unsafe {
                command.pre_exec(move || {
                    // New session/process group for whole-tree termination.
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if restricted {
                        let cpu_lim = libc::rlimit {
                            rlim_cur: cpu,
                            rlim_max: cpu + 5,
                        };
                        libc::setrlimit(libc::RLIMIT_CPU, &cpu_lim);
                        if mem_bytes > 0 {
                            let mem_lim = libc::rlimit {
                                rlim_cur: mem_bytes,
                                rlim_max: mem_bytes,
                            };
                            libc::setrlimit(libc::RLIMIT_AS, &mem_lim);
                        }
                        // RLIMIT_NPROC counts *all* processes of this UID,
                        // not just this tree; leave generous headroom above
                        // whatever is already running.
                        let mut current = libc::rlimit {
                            rlim_cur: 0,
                            rlim_max: 0,
                        };
                        if libc::getrlimit(libc::RLIMIT_NPROC, &mut current) == 0 {
                            let cap = current.rlim_max.clamp(1024, 8192);
                            let proc_lim = libc::rlimit {
                                rlim_cur: cap,
                                rlim_max: current.rlim_max,
                            };
                            libc::setrlimit(libc::RLIMIT_NPROC, &proc_lim);
                        }
                        // Prevent core dumps of possibly sensitive memory.
                        let core = libc::rlimit {
                            rlim_cur: 0,
                            rlim_max: 0,
                        };
                        libc::setrlimit(libc::RLIMIT_CORE, &core);
                    }
                    if want_netns && libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                        // Fail closed: if we promised network-off and cannot
                        // deliver it, refuse to run.
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let mut child = command.spawn().map_err(|e| {
            NexusError::Sandbox(format!("failed to spawn `{}`: {e}", spec.command_line()))
        })?;
        let child_pid = child.id();

        if let (Some(stdin_data), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(stdin_data.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cap = spec.output_hard_cap.max(4096);
        let live_out = live.clone();
        let stdout_task = tokio::spawn(read_capped(stdout, cap, live_out, false));
        let stderr_task = tokio::spawn(read_capped(stderr, cap, live, true));

        let timeout = Duration::from_secs(spec.timeout_secs.max(1));
        let (timed_out, status) = match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(status)) => (false, Some(status)),
            Ok(Err(e)) => {
                return Err(NexusError::Sandbox(format!("wait failed: {e}")));
            }
            Err(_) => {
                kill_process_group(child_pid);
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
        if output_capped {
            kill_process_group(child_pid);
        }
        // Ensure no orphaned process group survives.
        if timed_out || output_capped {
            kill_process_group(child_pid);
        }

        let isolation = self.isolation(spec.network);
        Ok(ExecOutcome {
            exit_code: status.and_then(|s| s.code()),
            stdout: stdout_text,
            stderr: stderr_text,
            duration_ms: start.elapsed().as_millis() as u64,
            timed_out,
            output_capped,
            backend: self.name().into(),
            isolation: isolation.level,
        })
    }
}

/// Read a child stream up to `cap` bytes, forwarding chunks to `live`.
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

fn kill_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // Negative pid = whole process group (created by setsid above).
        // SAFETY: plain syscall wrapper; no memory involved.
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
    use std::collections::BTreeMap;

    fn spec(program: &str, args: &[&str]) -> ExecSpec {
        ExecSpec {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            shell: false,
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            env_allowlist: vec!["PATH".into()],
            network: NetworkMode::Restricted,
            timeout_secs: 10,
            cpu_limit_secs: 10,
            memory_limit_mb: 512,
            output_hard_cap: 64 * 1024,
            stdin: None,
        }
    }

    #[tokio::test]
    async fn runs_simple_command() {
        let backend = ProcessBackend::new(false);
        let out = backend
            .execute(spec("echo", &["hello"]), None)
            .await
            .expect("exec");
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.stdout.trim(), "hello");
        assert!(!out.timed_out);
    }

    #[tokio::test]
    async fn enforces_timeout_and_kills_tree() {
        let backend = ProcessBackend::new(false);
        let mut s = spec("sleep", &["30"]);
        s.timeout_secs = 1;
        let start = std::time::Instant::now();
        let out = backend.execute(s, None).await.expect("exec");
        assert!(out.timed_out);
        assert!(start.elapsed().as_secs() < 10);
    }

    #[tokio::test]
    async fn caps_runaway_output() {
        let backend = ProcessBackend::new(false);
        let mut s = spec("/bin/sh", &["-c", "yes A 2>/dev/null | head -c 10000000"]);
        // shell=false here but /bin/sh -c as program+args is equivalent and
        // controlled by the test, not by model input.
        s.output_hard_cap = 10_000;
        s.timeout_secs = 15;
        let out = backend.execute(s, None).await.expect("exec");
        assert!(out.output_capped);
        assert!(out.stdout.len() <= 10_000);
    }

    #[tokio::test]
    async fn environment_is_scrubbed() {
        std::env::set_var("SNX_LEAK_TEST_SECRET_TOKEN", "leakme");
        let backend = ProcessBackend::new(false);
        let out = backend.execute(spec("env", &[]), None).await.expect("exec");
        assert!(!out.stdout.contains("leakme"));
        std::env::remove_var("SNX_LEAK_TEST_SECRET_TOKEN");
    }

    #[tokio::test]
    async fn network_off_blocks_when_namespaces_supported() {
        let backend = ProcessBackend::new(false);
        let _ = backend.availability().await;
        if !backend
            .netns_supported
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            // Kernel denies unprivileged namespaces; the isolation report
            // already communicates this honestly. Nothing to assert here.
            return;
        }
        let mut s = spec("/bin/sh", &["-c", "cat /proc/net/dev | tail -n +3 | wc -l"]);
        s.network = NetworkMode::Off;
        let out = backend.execute(s, None).await.expect("exec");
        // Inside a fresh netns only the loopback interface exists (1 line).
        let interfaces: i32 = out.stdout.trim().parse().unwrap_or(99);
        assert!(
            interfaces <= 1,
            "expected only loopback, got: {}",
            out.stdout
        );
    }

    #[tokio::test]
    async fn stdin_is_delivered() {
        let backend = ProcessBackend::new(false);
        let mut s = spec("cat", &[]);
        s.stdin = Some("piped-input".into());
        let out = backend.execute(s, None).await.expect("exec");
        assert_eq!(out.stdout, "piped-input");
    }
}
