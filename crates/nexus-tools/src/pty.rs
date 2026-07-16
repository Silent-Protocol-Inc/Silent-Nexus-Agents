//! PTY-backed interactive command execution.
//!
//! Some tools (REPLs, programs that detect a TTY, prompts) misbehave without a
//! pseudo-terminal. This module runs a command under a PTY, streaming output
//! and enforcing a timeout. It is exposed to the harness (used by the TUI's
//! interactive terminal view) rather than as a model tool, because a PTY
//! command cannot be pre-approved by exact text the way a batch command can.
//!
//! The PTY still runs under the workspace working directory with a scrubbed
//! environment; it does **not** run inside the container backend (a live TTY
//! into a container is a separate, deliberately unimplemented feature — see
//! docs/sandbox.md).

use nexus_core::{NexusError, Result};
use portable_pty::{CommandBuilder, PtySize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// Result of a PTY session.
#[derive(Debug, Clone)]
pub struct PtyOutcome {
    pub output: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
}

/// Run `program args…` under a PTY with a scrubbed environment. Blocking;
/// callers wrap in `spawn_blocking`.
pub fn run_pty(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: BTreeMap<String, String>,
    timeout_secs: u64,
    max_output: usize,
) -> Result<PtyOutcome> {
    let start = Instant::now();
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| NexusError::Sandbox(format!("openpty: {e}")))?;

    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(cwd);
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| NexusError::Sandbox(format!("spawn under pty: {e}")))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| NexusError::Sandbox(format!("pty reader: {e}")))?;

    let mut output: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let mut timed_out = false;
    let mut exit_code = None;
    loop {
        if start.elapsed() > Duration::from_secs(timeout_secs.max(1)) {
            let _ = child.kill();
            timed_out = true;
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = Some(status.exit_code() as i32);
                // Drain any remaining buffered output.
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    append_capped(&mut output, &buf[..n], max_output);
                    if output.len() >= max_output {
                        break;
                    }
                }
                break;
            }
            Ok(None) => {}
            Err(e) => return Err(NexusError::Sandbox(format!("pty wait: {e}"))),
        }
        match reader.read(&mut buf) {
            Ok(0) => std::thread::sleep(Duration::from_millis(20)),
            Ok(n) => {
                append_capped(&mut output, &buf[..n], max_output);
                if output.len() >= max_output {
                    let _ = child.kill();
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    Ok(PtyOutcome {
        output: nexus_core::sanitize::sanitize_terminal(&String::from_utf8_lossy(&output)),
        exit_code,
        timed_out,
        duration_ms: start.elapsed().as_millis() as u64,
    })
}

fn append_capped(out: &mut Vec<u8>, data: &[u8], cap: usize) {
    let remaining = cap.saturating_sub(out.len());
    out.extend_from_slice(&data[..data.len().min(remaining)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_captures_output() {
        let env: BTreeMap<String, String> = [(
            "PATH".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        )]
        .into_iter()
        .collect();
        let outcome = run_pty(
            "echo",
            &["pty-hello".to_string()],
            &std::env::temp_dir(),
            env,
            10,
            64_000,
        )
        .expect("pty run");
        assert!(outcome.output.contains("pty-hello"));
        assert_eq!(outcome.exit_code, Some(0));
    }
}
