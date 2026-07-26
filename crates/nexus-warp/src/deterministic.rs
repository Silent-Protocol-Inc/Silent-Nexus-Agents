//! Deterministic validation.
//!
//! Objective checks — build, tests, lint, schema — run in the candidate's
//! isolate. Each is a hard gate: a single failure vetoes the candidate and no
//! model verdict downstream can overturn it. Command execution is behind the
//! [`CheckRunner`] trait so the policy (which checks, and that failures veto) is
//! unit-tested without shelling out, while [`ProcessCheckRunner`] runs the real
//! commands for the live pipeline.

use crate::{ValidationReport, Verdict};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// The kind of objective check. All kinds are hard gates; the label drives
/// presentation and the default check sets per improvement plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Build,
    Test,
    Lint,
    Schema,
    Custom,
}

/// One objective check: a program to run and its arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub kind: CheckKind,
    pub program: String,
    pub args: Vec<String>,
}

impl Check {
    pub fn new(
        name: impl Into<String>,
        kind: CheckKind,
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// The result of running one check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub name: String,
    pub kind: CheckKind,
    pub passed: bool,
    pub detail: String,
    pub duration_ms: u64,
}

/// Executes checks. Implementors decide *how* a check runs; the validator decides
/// what a failure means (always a veto).
pub trait CheckRunner {
    fn run(&self, check: &Check, cwd: &Path) -> CheckOutcome;
}

/// Runs checks as real subprocesses in the isolate. Cargo checks are pinned to
/// this host's OOM-safe concurrency (the workspace test suite kills the box
/// otherwise), matching the release gate's environment.
pub struct ProcessCheckRunner;

impl CheckRunner for ProcessCheckRunner {
    fn run(&self, check: &Check, cwd: &Path) -> CheckOutcome {
        let started = Instant::now();
        let output = Command::new(&check.program)
            .args(&check.args)
            .current_dir(cwd)
            .env("CARGO_BUILD_JOBS", "2")
            .env("RUST_TEST_THREADS", "2")
            .output();
        let duration_ms = started.elapsed().as_millis() as u64;
        match output {
            Ok(out) => {
                let passed = out.status.success();
                // Keep a bounded tail of stderr for diagnostics; never the whole log.
                let stderr = String::from_utf8_lossy(&out.stderr);
                let detail = tail(stderr.trim(), 400);
                CheckOutcome {
                    name: check.name.clone(),
                    kind: check.kind,
                    passed,
                    detail,
                    duration_ms,
                }
            }
            Err(e) => CheckOutcome {
                name: check.name.clone(),
                kind: check.kind,
                passed: false,
                detail: format!("failed to launch `{}`: {e}", check.program),
                duration_ms,
            },
        }
    }
}

fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let start = text.len() - max;
    format!("…{}", &text[start..])
}

/// Runs a set of deterministic checks and produces a [`ValidationReport`]. Any
/// failed check is a hard failure and forces a `Rejected` verdict.
pub struct DeterministicValidator<R: CheckRunner> {
    runner: R,
}

impl<R: CheckRunner> DeterministicValidator<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    /// The default gate for a **code-plane** (Rust) candidate: it must compile,
    /// pass tests, and satisfy clippy before it can advance. Callers append a
    /// schema-no-drift check where the workspace supports it.
    pub fn code_plane_checks() -> Vec<Check> {
        vec![
            Check::new("cargo build", CheckKind::Build, "cargo", ["build", "-j2"]),
            Check::new("cargo test", CheckKind::Test, "cargo", ["test", "-j2"]),
            Check::new(
                "cargo clippy",
                CheckKind::Lint,
                "cargo",
                ["clippy", "-j2", "--all-targets", "--", "-D", "warnings"],
            ),
        ]
    }

    /// Run the checks in `cwd` and produce the report.
    pub fn validate(&self, candidate_id: &str, cwd: &Path, checks: &[Check]) -> ValidationReport {
        let mut report = ValidationReport::new(candidate_id, "deterministic");
        for check in checks {
            let outcome = self.runner.run(check, cwd);
            if !outcome.passed {
                report.hard_failures.push(format!(
                    "{}: {}",
                    outcome.name,
                    first_line(&outcome.detail)
                ));
            }
            report.checks.push(outcome);
        }
        report.verdict = if report.hard_failures.is_empty() {
            Verdict::Passed
        } else {
            Verdict::Rejected
        };
        report
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A runner that returns preset pass/fail per check name.
    struct MockRunner {
        results: BTreeMap<String, bool>,
    }

    impl CheckRunner for MockRunner {
        fn run(&self, check: &Check, _cwd: &Path) -> CheckOutcome {
            let passed = *self.results.get(&check.name).unwrap_or(&true);
            CheckOutcome {
                name: check.name.clone(),
                kind: check.kind,
                passed,
                detail: if passed { "ok".into() } else { "boom".into() },
                duration_ms: 1,
            }
        }
    }

    fn checks() -> Vec<Check> {
        DeterministicValidator::<ProcessCheckRunner>::code_plane_checks()
    }

    #[test]
    fn all_checks_passing_yields_passed() {
        let runner = MockRunner {
            results: BTreeMap::new(),
        };
        let validator = DeterministicValidator::new(runner);
        let report = validator.validate("cnd-1", Path::new("."), &checks());
        assert_eq!(report.verdict, Verdict::Passed);
        assert!(report.hard_failures.is_empty());
        assert_eq!(report.checks.len(), 3);
    }

    #[test]
    fn a_single_build_failure_vetoes_the_candidate() {
        let mut results = BTreeMap::new();
        results.insert("cargo build".to_string(), false);
        let validator = DeterministicValidator::new(MockRunner { results });
        let report = validator.validate("cnd-1", Path::new("."), &checks());
        assert_eq!(report.verdict, Verdict::Rejected);
        assert!(report
            .hard_failures
            .iter()
            .any(|f| f.contains("cargo build")));
    }

    #[test]
    fn process_runner_executes_real_commands() {
        let runner = ProcessCheckRunner;
        let validator = DeterministicValidator::new(runner);
        let pass = Check::new("true", CheckKind::Custom, "true", Vec::<String>::new());
        let fail = Check::new("false", CheckKind::Custom, "false", Vec::<String>::new());
        let report = validator.validate("cnd-1", Path::new("."), &[pass, fail]);
        assert_eq!(report.verdict, Verdict::Rejected);
        assert_eq!(report.checks.iter().filter(|c| c.passed).count(), 1);
        assert_eq!(report.checks.iter().filter(|c| !c.passed).count(), 1);
    }
}
