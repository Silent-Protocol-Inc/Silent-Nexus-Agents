use crate::OutputChunk;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, Notify};

pub(crate) struct OutputBudget {
    cap: usize,
    used: AtomicUsize,
    capped: AtomicBool,
    notify: Notify,
}

impl OutputBudget {
    pub(crate) fn new(cap: usize) -> Arc<Self> {
        Arc::new(Self {
            cap: cap.max(1),
            used: AtomicUsize::new(0),
            capped: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    fn claim(&self, requested: usize) -> usize {
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            if used >= self.cap {
                self.mark_capped();
                return 0;
            }
            let accepted = requested.min(self.cap - used);
            match self.used.compare_exchange(
                used,
                used + accepted,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    if accepted < requested || used + accepted >= self.cap {
                        self.mark_capped();
                    }
                    return accepted;
                }
                Err(actual) => used = actual,
            }
        }
    }

    fn mark_capped(&self) {
        if !self.capped.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }

    pub(crate) fn is_capped(&self) -> bool {
        self.capped.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_capped(&self) {
        loop {
            if self.is_capped() {
                return;
            }
            self.notify.notified().await;
        }
    }
}

pub(crate) async fn read_stream<R>(
    reader: Option<R>,
    budget: Arc<OutputBudget>,
    live: Option<mpsc::UnboundedSender<OutputChunk>>,
    is_stderr: bool,
) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return String::new();
    };
    let mut collected = Vec::new();
    let mut buffer = [0u8; 8_192];
    loop {
        let read = tokio::select! {
            result = reader.read(&mut buffer) => result,
            _ = budget.wait_capped() => break,
        };
        match read {
            Ok(0) => break,
            Ok(count) => {
                let accepted = budget.claim(count);
                if accepted > 0 {
                    collected.extend_from_slice(&buffer[..accepted]);
                    if let Some(sender) = &live {
                        let text = String::from_utf8_lossy(&buffer[..accepted]).to_string();
                        let _ = sender.send(if is_stderr {
                            OutputChunk::Stderr(text)
                        } else {
                            OutputChunk::Stdout(text)
                        });
                    }
                }
                if accepted < count || budget.is_capped() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&collected).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn shared_cap_cancels_a_silent_peer_reader() {
        let (mut flood_writer, flood_reader) = tokio::io::duplex(16_384);
        let (_silent_writer, silent_reader) = tokio::io::duplex(64);
        let budget = OutputBudget::new(1_024);

        let flood_task = tokio::spawn(read_stream(Some(flood_reader), budget.clone(), None, false));
        let silent_task =
            tokio::spawn(read_stream(Some(silent_reader), budget.clone(), None, true));
        flood_writer
            .write_all(&vec![b'x'; 2_048])
            .await
            .expect("write flood");

        let (flood, silent) = tokio::time::timeout(Duration::from_millis(250), async {
            (
                flood_task.await.expect("flood reader"),
                silent_task.await.expect("silent reader"),
            )
        })
        .await
        .expect("peer reader should stop as soon as the shared cap is reached");

        assert!(budget.is_capped());
        assert_eq!(flood.len(), 1_024);
        assert!(silent.is_empty());
    }
}
