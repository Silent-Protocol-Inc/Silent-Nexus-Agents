//! System diagnostics (`diag.*`). Read-only; secrets and sensitive
//! environment variables are never emitted.

use crate::{
    finalize_output, object_schema, Tool, ToolCategory, ToolContext, ToolMeta, ToolOutput,
    ToolRegistry,
};
use nexus_core::{Result, RiskLevel};
use nexus_policy::ActionRequest;
use serde_json::{json, Value};
use std::sync::Arc;

struct DiagTool {
    meta: ToolMeta,
}

pub fn register(registry: &mut ToolRegistry) {
    registry.register(Arc::new(DiagTool {
        meta: ToolMeta {
            name: "diag.system".into(),
            namespace: "diag".into(),
            description: "System diagnostics: OS, CPU count, memory, disk, GPU/accelerator, and detected runtime toolchains. Secrets are redacted.".into(),
            category: ToolCategory::Diagnostics,
            input_schema: object_schema(&[], &[]),
            output_schema: json!({"type": "string"}),
            risk: RiskLevel::Read,
            required_capabilities: vec!["diagnostics".into()],
            timeout_secs: 15,
            max_output_bytes: 16_000,
            deterministic: false,
            needs_network: false,
            needs_sandbox: false,
            side_effects: "none".into(),
        },
    }));
    registry.register(Arc::new(DiagTool {
        meta: ToolMeta {
            name: "diag.env".into(),
            namespace: "diag".into(),
            description: "List non-sensitive environment variable names available to the harness. Values of sensitive-looking variables are never shown.".into(),
            category: ToolCategory::Diagnostics,
            input_schema: object_schema(&[], &[]),
            output_schema: json!({"type": "string"}),
            risk: RiskLevel::Read,
            required_capabilities: vec!["diagnostics".into()],
            timeout_secs: 15,
            max_output_bytes: 16_000,
            deterministic: false,
            needs_network: false,
            needs_sandbox: false,
            side_effects: "none".into(),
        },
    }));
}

#[async_trait::async_trait]
impl Tool for DiagTool {
    fn meta(&self) -> &ToolMeta {
        &self.meta
    }

    fn action_request(&self, _args: &Value) -> Result<ActionRequest> {
        Ok(ActionRequest {
            tool: self.meta.name.clone(),
            risk: RiskLevel::Read,
            paths: vec![],
            command: None,
            command_analysis: None,
            destination: None,
            summary: self.meta.name.clone(),
        })
    }

    async fn execute(&self, ctx: &ToolContext, _args: Value) -> Result<ToolOutput> {
        if self.meta.name == "diag.env" {
            let mut names: Vec<String> = std::env::vars()
                .map(|(k, _)| k)
                .filter(|k| !nexus_core::redact::Redactor::is_sensitive_env_key(k))
                .collect();
            names.sort();
            let sensitive_count = std::env::vars()
                .filter(|(k, _)| nexus_core::redact::Redactor::is_sensitive_env_key(k))
                .count();
            let body = format!(
                "non-sensitive env vars ({}):\n{}\n\n[{} sensitive-looking variables hidden]",
                names.len(),
                names.join("\n"),
                sensitive_count
            );
            return finalize_output(ctx, &self.meta, body, json!({"count": names.len()})).await;
        }

        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let (mem_total, mem_avail) = read_meminfo();
        let disk = disk_free(ctx.workspace.root());
        let toolchains = detect_toolchains().await;
        let gpu = nexus_core::gpu::detect();
        let body = format!(
            "os: {os} ({arch})\ncpus: {cpus}\nmemory: {} MiB total, {} MiB available\nworkspace disk free: {}\ngpu: {}\ntoolchains: {}",
            mem_total / 1024,
            mem_avail / 1024,
            disk,
            gpu.summary(),
            if toolchains.is_empty() {
                "none detected".into()
            } else {
                toolchains.join(", ")
            }
        );
        finalize_output(
            ctx,
            &self.meta,
            body,
            json!({
                "os": os,
                "arch": arch,
                "cpus": cpus,
                "toolchains": toolchains,
                "gpu": {"has_gpu": gpu.has_gpu(), "accelerator": gpu.primary_backend(), "devices": gpu.gpus},
            }),
        )
        .await
    }
}

/// Return (total_kb, available_kb) from /proc/meminfo on Linux, else (0,0).
fn read_meminfo() -> (u64, u64) {
    #[cfg(target_os = "linux")]
    {
        let mut total = 0;
        let mut avail = 0;
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                match parts.next() {
                    Some("MemTotal:") => {
                        total = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    Some("MemAvailable:") => {
                        avail = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0)
                    }
                    _ => {}
                }
            }
        }
        (total, avail)
    }
    #[cfg(not(target_os = "linux"))]
    {
        (0, 0)
    }
}

fn disk_free(path: &std::path::Path) -> String {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        if let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: statvfs reads into a zeroed struct; no aliasing.
            #[allow(unsafe_code)]
            unsafe {
                let mut stat: libc::statvfs = std::mem::zeroed();
                if libc::statvfs(cpath.as_ptr(), &mut stat) == 0 {
                    let free = (stat.f_bavail as u64) * (stat.f_frsize as u64);
                    return format!("{} MiB", free / (1024 * 1024));
                }
            }
        }
    }
    "unknown".into()
}

async fn detect_toolchains() -> Vec<String> {
    let mut found = Vec::new();
    for (label, program, arg) in [
        ("rustc", "rustc", "--version"),
        ("cargo", "cargo", "--version"),
        ("node", "node", "--version"),
        ("python3", "python3", "--version"),
        ("go", "go", "version"),
        ("git", "git", "--version"),
    ] {
        if let Ok(out) = tokio::process::Command::new(program)
            .arg(arg)
            .output()
            .await
        {
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout);
                let v = v.lines().next().unwrap_or("").trim();
                found.push(format!("{label}: {v}"));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::context;

    #[tokio::test]
    async fn system_diag_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let mut r = ToolRegistry::new();
        register(&mut r);
        let out = r
            .get("diag.system")
            .expect("tool")
            .execute(&ctx, json!({}))
            .await
            .expect("exec");
        assert!(out.content.contains("cpus:"));
        assert!(out.content.contains("os:"));
    }

    #[tokio::test]
    async fn env_diag_hides_secrets() {
        std::env::set_var("SNX_DIAG_TEST_SECRET_KEY", "leak");
        std::env::set_var("SNX_DIAG_TEST_PLAIN", "ok");
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = context(dir.path());
        let mut r = ToolRegistry::new();
        register(&mut r);
        let out = r
            .get("diag.env")
            .expect("tool")
            .execute(&ctx, json!({}))
            .await
            .expect("exec");
        assert!(out.content.contains("SNX_DIAG_TEST_PLAIN"));
        assert!(!out.content.contains("SNX_DIAG_TEST_SECRET_KEY"));
        assert!(!out.content.contains("leak"));
        std::env::remove_var("SNX_DIAG_TEST_SECRET_KEY");
        std::env::remove_var("SNX_DIAG_TEST_PLAIN");
    }
}
