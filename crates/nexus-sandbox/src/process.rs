//! Approval-only host-process backend.
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

use crate::{
    ExecOutcome, ExecSpec, FilesystemAccess, IsolationReport, IsolationStrength, NetworkMode,
    OutputChunk, SandboxBackend,
};
use nexus_core::{NexusError, Result};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
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
            "host-unrestricted"
        } else {
            "approval-only-host"
        }
    }

    fn strength(&self) -> IsolationStrength {
        if self.unrestricted {
            IsolationStrength::None
        } else {
            IsolationStrength::ApprovalOnlyHost
        }
    }

    fn isolation(&self, network: NetworkMode) -> IsolationReport {
        if self.unrestricted {
            return IsolationReport {
                backend: self.name().into(),
                strength: IsolationStrength::None,
                level: "path-validation-only".into(),
                filesystem: "no isolation; workspace-boundary checks at tool layer only".into(),
                filesystem_access: FilesystemAccess::WorkspaceWrite,
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
            strength: IsolationStrength::ApprovalOnlyHost,
            level: "approval-only-host".into(),
            filesystem: "host filesystem visible; writes confined by workspace guard and OS permissions (not a read-only view)".into(),
            filesystem_access: FilesystemAccess::WorkspaceWrite,
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
            "approval-only host backend ready (network namespaces: {})",
            if netns { "supported" } else { "unavailable" }
        ))
    }

    async fn execute(
        &self,
        spec: ExecSpec,
        live: Option<mpsc::UnboundedSender<OutputChunk>>,
    ) -> Result<ExecOutcome> {
        if !spec.unsafe_host_approved {
            return Err(NexusError::ApprovalRequired(
                "host-process execution requires a fresh unsafe-host approval".into(),
            ));
        }
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
            && spec.effective_network() == NetworkMode::Off
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
        let budget = crate::output::OutputBudget::new(spec.output_hard_cap);
        let live_out = live.clone();
        let stdout_task = tokio::spawn(crate::output::read_stream(
            stdout,
            budget.clone(),
            live_out,
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
                kill_process_group(child_pid);
                let _ = child.kill().await;
                let _ = child.wait().await;
                (true, None)
            }
            Completion::OutputCap => {
                kill_process_group(child_pid);
                let _ = child.kill().await;
                let _ = child.wait().await;
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
        // Ensure no orphaned process group survives.
        if timed_out || output_capped {
            kill_process_group(child_pid);
        }

        let isolation = self.isolation(spec.effective_network());
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
            approved_network: NetworkMode::Restricted,
            filesystem_access: FilesystemAccess::WorkspaceWrite,
            sensitive_path_masks: Vec::new(),
            unsafe_host_approved: true,
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
        let start = Instant::now();
        let out = backend.execute(s, None).await.expect("exec");
        assert!(out.output_capped);
        assert!(out.stdout.len() + out.stderr.len() <= 10_000);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_cap_kills_the_entire_process_group() {
        let directory = tempfile::tempdir().expect("directory");
        let pid_file = directory.path().join("child.pid");
        let script = format!(
            "sleep 30 & child=$!; printf '%s' \"$child\" > '{}'; yes flood",
            pid_file.display()
        );
        let backend = ProcessBackend::new(false);
        let mut command = spec("/bin/sh", &["-c", &script]);
        command.output_hard_cap = 8_192;
        command.timeout_secs = 30;
        let started = Instant::now();
        let outcome = backend.execute(command, None).await.expect("execute");
        assert!(outcome.output_capped);
        assert!(started.elapsed() < Duration::from_secs(2));

        let child_pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("pid file")
            .parse()
            .expect("pid");
        for _ in 0..20 {
            #[allow(unsafe_code)]
            let alive = unsafe { libc::kill(child_pid, 0) == 0 };
            if !alive {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("child process {child_pid} survived the output-cap kill");
    }

    #[tokio::test]
    async fn host_execution_requires_one_time_approval_state() {
        let backend = ProcessBackend::new(false);
        let mut command = spec("echo", &["blocked"]);
        command.unsafe_host_approved = false;
        assert!(matches!(
            backend.execute(command, None).await,
            Err(NexusError::ApprovalRequired(_))
        ));
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
