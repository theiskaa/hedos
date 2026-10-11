//! Stream helpers shared by the Python-sidecar adapters: bridging a sidecar
//! stream into a runtime stream, and think-splitting a text stream.

use kernel::capabilities::{CapabilityChunk, Piece, ThinkSplitter};
use tokio::sync::mpsc;

use super::{ChunkStream, RuntimeError, RuntimeStream};
use crate::sidecar::SidecarStream;

/// Forward a sidecar stream as a runtime stream, mapping sidecar errors. Dropping
/// the returned stream drops the sidecar stream at once, triggering its
/// cancellation, rather than at the sidecar's next event, which a long silent
/// stage (a model loading, a song synthesizing) may not send for minutes.
/// Generic over the item type so both capability-chunk and job-event streams
/// share it.
pub(crate) fn bridge<T: Send + 'static>(mut sidecar: SidecarStream<T>) -> RuntimeStream<T> {
    let (tx, stream) = RuntimeStream::channel();
    tokio::spawn(async move {
        loop {
            let item = tokio::select! {
                item = sidecar.recv() => item,
                () = tx.closed() => break,
            };
            let Some(item) = item else { break };
            if tx.send(item.map_err(RuntimeError::from)).is_err() {
                break;
            }
        }
    });
    stream
}

/// Run the stream's visible text through a [`ThinkSplitter`], emitting `Thinking`
/// chunks for reasoning delimited by think tags. `Done` flushes any pending text
/// (then carries its stats through); other chunks pass through unchanged.
pub(crate) fn separating(mut upstream: ChunkStream) -> ChunkStream {
    let (tx, stream) = RuntimeStream::channel();
    tokio::spawn(async move {
        let mut splitter = ThinkSplitter::new();
        while let Some(item) = upstream.recv().await {
            match item {
                Ok(CapabilityChunk::Text(text)) => {
                    if !drain_pieces(&tx, splitter.feed(&text)) {
                        return;
                    }
                }
                Ok(CapabilityChunk::Done(stats)) => {
                    if !drain_pieces(&tx, splitter.flush()) {
                        return;
                    }
                    if tx.send(Ok(CapabilityChunk::Done(stats))).is_err() {
                        return;
                    }
                }
                Ok(other) => {
                    if tx.send(Ok(other)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                    return;
                }
            }
        }
        let _ = drain_pieces(&tx, splitter.flush());
    });
    stream
}

/// Send each [`Piece`] as its corresponding chunk; returns `false` if the
/// receiver has gone away.
fn drain_pieces(
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
    pieces: Vec<Piece>,
) -> bool {
    for piece in pieces {
        let chunk = match piece {
            Piece::Text(value) => CapabilityChunk::Text(value),
            Piece::Thinking(value) => CapabilityChunk::Thinking(value),
        };
        if tx.send(Ok(chunk)).is_err() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn separating_splits_thinking_out_of_the_text() {
        let (tx, upstream) = RuntimeStream::channel();
        tx.send(Ok(CapabilityChunk::Text(
            "hi <think>reasoning</think> bye".to_owned(),
        )))
        .unwrap();
        tx.send(Ok(CapabilityChunk::Done(None))).unwrap();
        drop(tx);

        let mut out = separating(upstream);
        let mut texts = Vec::new();
        let mut thinking = Vec::new();
        let mut done = false;
        while let Some(item) = out.recv().await {
            match item.unwrap() {
                CapabilityChunk::Text(text) => texts.push(text),
                CapabilityChunk::Thinking(text) => thinking.push(text),
                CapabilityChunk::Done(_) => done = true,
                _ => {}
            }
        }
        assert!(thinking.iter().any(|text| text.contains("reasoning")));
        let visible = texts.concat();
        assert!(visible.contains("hi") && visible.contains("bye"));
        assert!(!visible.contains("reasoning"));
        assert!(done);
    }

    #[tokio::test]
    async fn dropping_the_bridged_stream_lets_a_silent_sidecar_go_at_once() {
        let (sidecar_tx, rx) =
            mpsc::unbounded_channel::<Result<u8, crate::sidecar::SidecarError>>();
        let bridged = bridge(crate::sidecar::SidecarStream { rx });
        drop(bridged);
        // The sidecar has sent nothing, yet its stream is released.
        tokio::time::timeout(std::time::Duration::from_secs(2), sidecar_tx.closed())
            .await
            .expect("released without waiting for a sidecar event");
    }
}
