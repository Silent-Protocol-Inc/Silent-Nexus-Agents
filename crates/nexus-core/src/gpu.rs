//! Host GPU / accelerator detection.
//!
//! Pure and subprocess-free: detection reads only sysfs/procfs (Linux) and
//! platform constants, so it is fast, side-effect-free, and safe to call from
//! anywhere (including provider construction). It reports honestly — an absent
//! or unreadable GPU yields "none detected (CPU-only)", never a guess, which
//! matches Silent Nexus's CPU-first stance.
//!
//! What it can and cannot know:
//!  * It detects the *presence* and vendor of installed GPUs, and the compute
//!    backend they would use (CUDA / ROCm / Metal / oneAPI).
//!  * It reports VRAM only where the OS exposes it without a vendor tool
//!    (AMD via sysfs). NVIDIA VRAM is left unknown here (querying it needs
//!    `nvidia-smi`), and that is stated rather than fabricated.
//!  * Whether a *given model server* actually offloads to the GPU is a separate,
//!    provider-specific question (e.g. Ollama's `/api/ps` VRAM figures).

use serde::{Deserialize, Serialize};

/// A single detected GPU.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gpu {
    /// Vendor label, e.g. `NVIDIA`, `AMD`, `Intel`, `Apple`.
    pub vendor: String,
    /// Human-readable model name when known, else a vendor-based fallback.
    pub name: String,
    /// Total video memory in MiB, when the OS exposes it without a vendor tool.
    pub memory_mb: Option<u64>,
    /// Compute backend the model runtime would use, e.g. `CUDA`, `ROCm`,
    /// `Metal`, `oneAPI`.
    pub backend: String,
    /// True for integrated GPUs (shared system memory) when known.
    pub integrated: bool,
}

/// The result of a host GPU scan.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GpuReport {
    pub gpus: Vec<Gpu>,
    /// How detection was performed and any honest caveats.
    pub notes: Vec<String>,
}

impl GpuReport {
    /// Whether at least one GPU was detected.
    pub fn has_gpu(&self) -> bool {
        !self.gpus.is_empty()
    }

    /// The compute backend of the first discrete GPU, else the first GPU, else
    /// `None`. Used to advertise a local model's acceleration.
    pub fn primary_backend(&self) -> Option<&str> {
        self.gpus
            .iter()
            .find(|g| !g.integrated)
            .or_else(|| self.gpus.first())
            .map(|g| g.backend.as_str())
    }

    /// A one-line human summary.
    pub fn summary(&self) -> String {
        if self.gpus.is_empty() {
            return "none detected (CPU-only)".to_string();
        }
        self.gpus
            .iter()
            .map(|g| {
                let mem = g
                    .memory_mb
                    .map(|m| format!(", {} MiB", m))
                    .unwrap_or_default();
                let kind = if g.integrated { " integrated" } else { "" };
                format!("{} {}{} [{}{}]", g.vendor, g.name, kind, g.backend, mem)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Map a PCI vendor id (e.g. `0x10de`) to a (vendor, backend) pair.
fn classify_vendor(vendor_id: &str) -> Option<(&'static str, &'static str)> {
    match vendor_id.trim().to_lowercase().trim_start_matches("0x") {
        "10de" => Some(("NVIDIA", "CUDA")),
        "1002" | "1022" => Some(("AMD", "ROCm/Vulkan")),
        "8086" => Some(("Intel", "oneAPI/Vulkan")),
        "13b5" => Some(("ARM", "Vulkan")),
        "5143" => Some(("Qualcomm", "Vulkan")),
        _ => None,
    }
}

/// Extract the `Model:` line from an NVIDIA `/proc` information file.
fn parse_nvidia_model(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("Model:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Convert a sysfs VRAM byte count string to MiB.
fn vram_mb_from_bytes(bytes_str: &str) -> Option<u64> {
    bytes_str
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|b| *b > 0)
        .map(|b| b / (1024 * 1024))
}

/// Detect host GPUs.
pub fn detect() -> GpuReport {
    #[cfg(target_os = "linux")]
    {
        detect_linux()
    }
    #[cfg(target_os = "macos")]
    {
        detect_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        GpuReport {
            gpus: vec![],
            notes: vec![format!(
                "GPU detection is not implemented on {}; assuming CPU-only",
                std::env::consts::OS
            )],
        }
    }
}

#[cfg(target_os = "macos")]
fn detect_macos() -> GpuReport {
    // Pure heuristic without a subprocess: Apple Silicon always has an
    // integrated Metal GPU. Intel Macs may have a discrete GPU, but confirming
    // that needs `system_profiler`, so we report honestly rather than guess.
    if std::env::consts::ARCH == "aarch64" {
        GpuReport {
            gpus: vec![Gpu {
                vendor: "Apple".into(),
                name: "Apple Silicon GPU".into(),
                memory_mb: None,
                backend: "Metal".into(),
                integrated: true,
            }],
            notes: vec!["unified memory; VRAM shared with system RAM".into()],
        }
    } else {
        GpuReport {
            gpus: vec![],
            notes: vec![
                "Intel Mac: discrete-GPU detection needs `system_profiler`; assuming CPU/Metal-integrated".into(),
            ],
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_linux() -> GpuReport {
    use std::path::Path;
    let mut gpus = Vec::new();
    let mut notes = Vec::new();

    // NVIDIA model names via procfs (no subprocess).
    let mut nvidia_names: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc/driver/nvidia/gpus") {
        for e in entries.flatten() {
            if let Ok(text) = std::fs::read_to_string(e.path().join("information")) {
                if let Some(name) = parse_nvidia_model(&text) {
                    nvidia_names.push(name);
                }
            }
        }
    }
    let mut nvidia_idx = 0usize;

    // Scan render nodes under /sys/class/drm (card0, card1, …), skipping the
    // connector sub-devices (card0-DP-1, …).
    let drm = Path::new("/sys/class/drm");
    if let Ok(entries) = std::fs::read_dir(drm) {
        let mut cards: Vec<_> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.starts_with("card") && !n.contains('-'))
            .collect();
        cards.sort();
        for card in cards {
            let dev = drm.join(&card).join("device");
            let Ok(vendor_id) = std::fs::read_to_string(dev.join("vendor")) else {
                continue;
            };
            let Some((vendor, backend)) = classify_vendor(&vendor_id) else {
                continue;
            };
            // VRAM: AMD exposes total VRAM via sysfs; others usually do not.
            let memory_mb = std::fs::read_to_string(dev.join("mem_info_vram_total"))
                .ok()
                .and_then(|s| vram_mb_from_bytes(&s));
            // Name: prefer a vendor-specific source.
            let name = if vendor == "NVIDIA" && nvidia_idx < nvidia_names.len() {
                let n = nvidia_names[nvidia_idx].clone();
                nvidia_idx += 1;
                n
            } else {
                std::fs::read_to_string(dev.join("product_name"))
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("{vendor} GPU"))
            };
            // Intel display controllers are typically integrated.
            let integrated = vendor == "Intel" && memory_mb.is_none();
            gpus.push(Gpu {
                vendor: vendor.into(),
                name,
                memory_mb,
                backend: backend.into(),
                integrated,
            });
        }
    } else {
        notes.push("/sys/class/drm is unreadable; cannot enumerate GPUs".into());
    }

    if gpus.iter().any(|g| g.vendor == "NVIDIA") {
        notes.push(
            "NVIDIA VRAM not shown (needs `nvidia-smi`); presence confirmed via /proc".into(),
        );
    }
    if gpus.is_empty() && notes.is_empty() {
        notes.push("no GPU found under /sys/class/drm (CPU-only)".into());
    }

    GpuReport { gpus, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_vendors() {
        assert_eq!(classify_vendor("0x10de"), Some(("NVIDIA", "CUDA")));
        assert_eq!(classify_vendor("0x1002"), Some(("AMD", "ROCm/Vulkan")));
        assert_eq!(classify_vendor("0x8086"), Some(("Intel", "oneAPI/Vulkan")));
        assert!(classify_vendor("0xbeef").is_none());
    }

    #[test]
    fn parses_nvidia_model_line() {
        let info = "Model: \t NVIDIA GeForce RTX 4090\nIRQ: 42\nBus Location: 0000:01:00.0\n";
        assert_eq!(
            parse_nvidia_model(info).as_deref(),
            Some("NVIDIA GeForce RTX 4090")
        );
        assert!(parse_nvidia_model("IRQ: 42").is_none());
    }

    #[test]
    fn converts_vram_bytes_to_mib() {
        assert_eq!(vram_mb_from_bytes("17163091968"), Some(16368));
        assert_eq!(vram_mb_from_bytes("0"), None);
        assert_eq!(vram_mb_from_bytes("garbage"), None);
    }

    #[test]
    fn report_summary_and_backend() {
        let report = GpuReport {
            gpus: vec![Gpu {
                vendor: "NVIDIA".into(),
                name: "RTX 4090".into(),
                memory_mb: Some(24564),
                backend: "CUDA".into(),
                integrated: false,
            }],
            notes: vec![],
        };
        assert!(report.has_gpu());
        assert_eq!(report.primary_backend(), Some("CUDA"));
        assert!(report.summary().contains("NVIDIA RTX 4090"));

        let empty = GpuReport::default();
        assert!(!empty.has_gpu());
        assert_eq!(empty.primary_backend(), None);
        assert_eq!(empty.summary(), "none detected (CPU-only)");
    }

    #[test]
    fn detect_runs_without_panicking() {
        // On CI/sandbox hosts this typically reports CPU-only; it must never panic.
        let r = detect();
        let _ = r.summary();
        let _ = r.has_gpu();
    }
}
