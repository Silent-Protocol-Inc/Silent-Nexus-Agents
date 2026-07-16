//! Deterministic mock backend for tests: records specs, returns scripted
//! outcomes, and never touches the host.

use crate::{
    ExecOutcome, ExecSpec, FilesystemAccess, IsolationReport, IsolationStrength, NetworkMode,
    OutputChunk, SandboxBackend,
};
use nexus_core::Result;
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub struct MockBackend {
    outcomes: Mutex<VecDeque<ExecOutcome>>,
    pub executed: Mutex<Vec<ExecSpec>>,
}

impl MockBackend {
    pub fn new(outcomes: Vec<ExecOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            executed: Mutex::new(Vec::new()),
        }
    }

    pub fn success(stdout: &str) -> ExecOutcome {
        ExecOutcome {
            exit_code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
            duration_ms: 1,
            timed_out: false,
            output_capped: false,
            backend: "mock".into(),
            isolation: "mock".into(),
        }
    }

    pub fn failure(stderr: &str, code: i32) -> ExecOutcome {
        ExecOutcome {
            exit_code: Some(code),
            stdout: String::new(),
            stderr: stderr.to_string(),
            duration_ms: 1,
            timed_out: false,
            output_capped: false,
            backend: "mock".into(),
            isolation: "mock".into(),
        }
    }
}

#[async_trait::async_trait]
impl SandboxBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn strength(&self) -> IsolationStrength {
        IsolationStrength::Mock
    }

    fn isolation(&self, _network: NetworkMode) -> IsolationReport {
        IsolationReport {
            backend: "mock".into(),
            strength: IsolationStrength::Mock,
            level: "mock".into(),
            filesystem: "none (test double)".into(),
            filesystem_access: FilesystemAccess::NoWorkspace,
            network: "none (test double)".into(),
            resources: "none (test double)".into(),
            caveats: vec!["test backend; never use outside tests".into()],
        }
    }

    async fn availability(&self) -> std::result::Result<String, String> {
        Ok("mock backend always available".into())
    }

    async fn execute(
        &self,
        spec: ExecSpec,
        live: Option<mpsc::UnboundedSender<OutputChunk>>,
    ) -> Result<ExecOutcome> {
        if let Ok(mut executed) = self.executed.lock() {
            executed.push(spec);
        }
        let outcome = self
            .outcomes
            .lock()
            .ok()
            .and_then(|mut o| o.pop_front())
            .unwrap_or_else(|| Self::success(""));
        if let Some(tx) = live {
            if !outcome.stdout.is_empty() {
                let _ = tx.send(OutputChunk::Stdout(outcome.stdout.clone()));
            }
            if !outcome.stderr.is_empty() {
                let _ = tx.send(OutputChunk::Stderr(outcome.stderr.clone()));
            }
        }
        Ok(outcome)
    }
}
