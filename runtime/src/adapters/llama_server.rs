//! The local llama.cpp adapter, served through `llama-server` rather than an
//! in-process FFI engine. `llama-server` speaks the OpenAI API, so this adapter
//! ensures a server is running for the model's GGUF, then proxies the request
//! through the shared OpenAI streaming path. A GGUF that only embeds is served
//! from its own embedding-mode server over `/v1/embeddings`; a chat model is
//! never asked to embed, nor an embedder to chat.
//!
//! Rather than running llama.cpp in-process over a Metal FFI binding, this takes
//! the subprocess route and avoids the FFI: the non-streaming surface
//! (bid/can_serve/context/honored params) lives here, and the engine is a
//! [`LlamaBackend`] that hands back a running server's base URL.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use kernel::records::{BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId};
use kernel::resolution::{GgufPooling, IdentifiedModel, ModelFormat, RuntimeBid, gguf_facts};

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use tokio::sync::mpsc;

use super::openai::{
    EmbeddingsFailure, OPTION_KEYS, embeddings_body, post_embeddings, request_body,
    stream_completions,
};
use super::{ChunkStream, RuntimeAdapter, RuntimeError};

const DEFAULT_CONTEXT: i64 = 4096;
const MAX_DEFAULT_CONTEXT: i64 = 32768;
const MIN_CONTEXT: i64 = 512;

/// The widest window an embedding server is launched with. An encoder reads a
/// whole input in one physical batch, so the batch is the window, and both
/// cost memory whatever the input: Qwen3-Embedding-0.6B declares 32768 and
/// idles near 4.4 GB there against 1.6 GB at 8192, and one 8192-token input
/// through a bge-m3-sized encoder peaks near 9 GB. 8192 is the window bge-m3
/// and jina v2 are trained with, so the long-input encoders lose nothing.
const MAX_EMBEDDING_CONTEXT: i64 = 8192;

/// The embedding architectures llama.cpp keeps no KV cache for
/// (`llama_model::create_memory` returns none). `llama-embed` and `t5encoder`
/// embed too, but get a cache like any decoder.
const MEMORYLESS_EMBEDDERS: [&str; 10] = [
    "bert",
    "jina-bert-v2",
    "jina-bert-v3",
    "nomic-bert",
    "nomic-bert-moe",
    "neo-bert",
    "eurobert",
    "modern-bert",
    "gemma-embedding",
    "gemma-embedding2",
];

/// How many inputs one request to an embedding server carries at most.
/// llama-server writes a float in up to about 20 characters, so 64 vectors of
/// even 8192 dimensions come to about 10 MiB, a third of the response cap.
const EMBEDDING_CHUNK_INPUTS: usize = 64;

/// The bytes of input one request to an embedding server carries at most, per
/// token of the server's window. Text runs at about four bytes a token, a
/// little more for English prose and a little less for code or CJK, so a
/// request holds about one window of tokens. On a one-slot decoder that is a
/// few seconds of work (Qwen3-Embedding-0.6B reads about 3000 tokens a second
/// of long inputs on an M-series Mac), which bounds how long another request
/// to the same model waits behind a batch; 64 long inputs at once took 40 s.
/// An input longer than the budget still goes, alone.
const EMBEDDING_CHUNK_BYTES_PER_TOKEN: usize = 4;

/// A future returning a running server's OpenAI base URL (or why it couldn't be
/// started).
pub type BackendFuture = Pin<Box<dyn Future<Output = Result<String, RuntimeError>> + Send>>;

/// What a `llama-server` is launched to do. llama-server serves embeddings
/// only from a server started for them, so one model's chat and embedding
/// servers are never the same process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMode {
    /// Chat and text completion over `/v1/chat/completions`.
    Chat,
    /// Embeddings over `/v1/embeddings`, started with `--embedding`.
    Embedding,
}

/// A file's modification time and size, which change when it is rewritten.
type FileSignature = (SystemTime, u64);

/// How an embedding GGUF reads its input, as its header says. llama.cpp
/// launches and bounds the two kinds differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbedderKind {
    /// An encoder llama.cpp keeps no memory for (BERT, nomic-bert,
    /// gemma-embedding), so its server's slots share nothing.
    Encoder,
    /// Any other embedder: a decoder converted to embed (Qwen3-Embedding), or
    /// an encoder llama.cpp still gives a KV cache (`llama-embed`,
    /// `t5encoder`). Its server's slots share that cache, and `pools_last`
    /// says it reads its vector off the last token, as Qwen3-Embedding does.
    Decoder {
        /// Whether the header pools by the last token.
        pools_last: bool,
    },
}

impl EmbedderKind {
    /// The kind of the GGUF at `path`, read from its header. A header that
    /// cannot be read is taken for a decoder that does not pool by the last
    /// token, which launches safely and claims no less than the window.
    pub(crate) fn of(path: &str) -> Self {
        match gguf_facts(Path::new(path)) {
            Some(facts) => Self::from_header(facts.architecture.as_deref(), facts.pooling),
            None => Self::Decoder { pools_last: false },
        }
    }

    /// The kind a header naming `architecture` and `pooling` describes: an
    /// encoder when llama.cpp's `create_memory` gives the architecture no
    /// memory, a decoder otherwise.
    pub(crate) fn from_header(architecture: Option<&str>, pooling: Option<GgufPooling>) -> Self {
        if architecture.is_some_and(|name| MEMORYLESS_EMBEDDERS.contains(&name)) {
            Self::Encoder
        } else {
            Self::Decoder {
                pools_last: pooling == Some(GgufPooling::Last),
            }
        }
    }

    /// The most tokens one input may hold on a server launched with `window`.
    /// llama-server reads a decoder that pools by the last token in pieces
    /// and refuses an input that fills the whole context, so it takes one
    /// token less; anything else is read whole, up to the window.
    pub(crate) fn most_tokens(self, window: i64) -> i64 {
        match self {
            Self::Decoder { pools_last: true } => window - 1,
            Self::Encoder | Self::Decoder { pools_last: false } => window,
        }
    }
}

impl ServerMode {
    /// The mode `record` is served in: embedding for a record that claims
    /// `embed` and neither chat nor completion; chat for one that claims chat
    /// or completion, or nothing at all (a pin over an unidentified file);
    /// and none for a record whose claims no mode answers.
    pub(crate) fn of(record: &ModelRecord) -> Option<Self> {
        let claims = |capability: Capability| record.capabilities.contains(&capability);
        if claims(Capability::chat()) || claims(Capability::complete()) {
            Some(Self::Chat)
        } else if claims(Capability::embed()) {
            Some(Self::Embedding)
        } else if record.capabilities.is_empty() {
            Some(Self::Chat)
        } else {
            None
        }
    }

    /// Whether a server in this mode answers `capability`.
    pub(crate) fn serves(self, capability: &Capability) -> bool {
        match self {
            Self::Chat => {
                *capability == Capability::chat() || *capability == Capability::complete()
            }
            Self::Embedding => *capability == Capability::embed(),
        }
    }
}

/// Ensures a local OpenAI-compatible server is running for a model and returns its
/// base URL. The production implementation spawns and supervises `llama-server`
/// (a separable unit); this trait is the seam the adapter drives, so it can run
/// against any such server — real or mocked.
pub trait LlamaBackend: Send + Sync {
    /// Ensure a server is running for `record` in `mode`, sized to
    /// `context_tokens`, and return its base URL (e.g. `http://127.0.0.1:8080`).
    fn base_url(
        &self,
        record: &ModelRecord,
        context_tokens: i64,
        mode: ServerMode,
    ) -> BackendFuture;
}

/// Serves local GGUF models by proxying to a `llama-server` instance.
pub struct LlamaServerAdapter {
    id: RuntimeId,
    backend: Arc<dyn LlamaBackend>,
    client: reqwest::Client,
    /// Each embedder GGUF's kind, by path, with the modification time and
    /// size it was read at, so `/v1/models` does not read every embedder's
    /// header on each call.
    kinds: Mutex<HashMap<String, (FileSignature, EmbedderKind)>>,
}

impl LlamaServerAdapter {
    /// An adapter over `backend`.
    pub fn new(backend: Arc<dyn LlamaBackend>) -> Self {
        Self {
            id: RuntimeId::llama_cpp(),
            backend,
            client: reqwest::Client::new(),
            kinds: Mutex::new(HashMap::new()),
        }
    }

    /// The kind of the embedder GGUF at `path`, read again only when the file
    /// changed since it was last read.
    fn embedder_kind(&self, path: &str) -> EmbedderKind {
        let Some(signature) = std::fs::metadata(path)
            .ok()
            .and_then(|meta| Some((meta.modified().ok()?, meta.len())))
        else {
            return EmbedderKind::of(path);
        };
        if let Ok(kinds) = self.kinds.lock()
            && let Some((seen, kind)) = kinds.get(path)
            && *seen == signature
        {
            return *kind;
        }
        let kind = EmbedderKind::of(path);
        if let Ok(mut kinds) = self.kinds.lock() {
            kinds.insert(path.to_owned(), (signature, kind));
        }
        kind
    }

    /// Embed the payload's `input` through an embedding-mode server sized to
    /// the encoder's own window, capped at [`MAX_EMBEDDING_CONTEXT`]. A
    /// requested `context_length` is ignored: an encoder reads at most the
    /// window it was trained with. A batch goes to the server in chunks (see
    /// [`chunked`]), so no one response outgrows the cap however wide the
    /// vectors are and no one request holds the server for long; their
    /// vectors come back in input order and their token counts are summed. A
    /// consumer that goes away drops the request in flight, which llama-server
    /// takes as a cancel, and nothing more is sent.
    fn invoke_embed(&self, record: &ModelRecord, payload: JsonValue) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let window = Self::embedding_context_tokens(record);
        let model = wire_model_name(record);
        let backend = Arc::clone(&self.backend);
        let client = self.client.clone();
        let record = record.clone();

        tokio::spawn(async move {
            let inputs = match embed_inputs(&payload) {
                Ok(inputs) => inputs,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };
            let starting = backend.base_url(&record, window, ServerMode::Embedding);
            let base = tokio::select! {
                result = starting => match result {
                    Ok(base) => base,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                },
                _ = tx.closed() => return,
            };
            let server = EmbeddingServer {
                client: &client,
                base: &base,
                model: &model,
                name: &record.name,
                window,
            };
            let batch = inputs.len() > 1;
            let mut prompt_tokens = Some(0i64);
            for range in chunked(&inputs, window) {
                let chunk = &inputs[range.clone()];
                let Some(embedded) = unless_abandoned(&tx, server.embed(chunk)).await else {
                    return;
                };
                let read = match embedded {
                    Ok(read) => Ok(read),
                    Err(Refused::TooLong(too_long)) if batch && chunk.len() == 1 => {
                        Err(too_long.rejected(Some(range.start)))
                    }
                    Err(Refused::TooLong(too_long)) if batch => {
                        let located = server.locate(chunk, range.start, too_long);
                        let Some(err) = unless_abandoned(&tx, located).await else {
                            return;
                        };
                        Err(err)
                    }
                    Err(Refused::TooLong(too_long)) => Err(too_long.rejected(None)),
                    Err(Refused::Failed(err)) => Err(err),
                };
                let (vectors, tokens) = match read {
                    Ok((vectors, tokens)) if vectors.len() == chunk.len() => (vectors, tokens),
                    Ok((vectors, _)) => {
                        let _ = tx.send(Err(RuntimeError::Failed(format!(
                            "llama-server returned {} embeddings for {} inputs",
                            vectors.len(),
                            chunk.len()
                        ))));
                        return;
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                };
                // A total is reported only when every chunk reported its count.
                prompt_tokens = prompt_tokens
                    .zip(tokens)
                    .map(|(sum, tokens)| sum.saturating_add(tokens));
                for vector in vectors {
                    if tx.send(Ok(CapabilityChunk::Vector(vector))).is_err() {
                        return;
                    }
                }
            }
            let stats = GenerationStats {
                prompt_tokens,
                ..Default::default()
            };
            let _ = tx.send(Ok(CapabilityChunk::Done(Some(stats))));
        });
        stream
    }

    /// The window an embedding server for `record` is launched with: its
    /// declared window, capped at [`MAX_EMBEDDING_CONTEXT`].
    fn embedding_context_tokens(record: &ModelRecord) -> i64 {
        Self::effective_context_tokens(record, None).min(MAX_EMBEDDING_CONTEXT)
    }

    /// The effective context window: the requested size (or a capped default)
    /// clamped into `[min(512, base), base]`, where `base` is the model's declared
    /// context length or 4096.
    pub fn effective_context_tokens(record: &ModelRecord, requested: Option<i64>) -> i64 {
        let base = record
            .context_length
            .filter(|length| *length > 0)
            .unwrap_or(DEFAULT_CONTEXT);
        let capped_default = base.min(MAX_DEFAULT_CONTEXT);
        let lower = base.min(MIN_CONTEXT);
        requested.unwrap_or(capped_default).max(lower).min(base)
    }
}

impl RuntimeAdapter for LlamaServerAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        true
    }

    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        record.runtime.id.as_ref() == Some(&self.id)
            && ServerMode::of(record).is_some_and(|mode| mode.serves(capability))
    }

    /// A GGUF that chats, or one that embeds. A GGUF that claims neither (a
    /// projector, a vocoder, a model whose header pools nothing or ranks) draws
    /// no bid, since no server mode would answer for it.
    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        let servable = identified.capabilities.contains(&Capability::chat())
            || identified.capabilities.contains(&Capability::embed());
        if identified.format != ModelFormat::Gguf || !servable {
            return None;
        }
        Some(RuntimeBid::new(RunTier::Native, BidPreference::LLAMA_CPP))
    }

    fn effective_context_window(
        &self,
        record: &ModelRecord,
        requested: Option<i64>,
    ) -> Option<i64> {
        Some(match ServerMode::of(record) {
            Some(ServerMode::Embedding) => self
                .embedder_kind(model_gguf_path(record))
                .most_tokens(Self::embedding_context_tokens(record)),
            _ => Self::effective_context_tokens(record, requested),
        })
    }

    fn invoke(
        &self,
        record: &ModelRecord,
        capability: Capability,
        payload: JsonValue,
    ) -> ChunkStream {
        if capability == Capability::embed() {
            return self.invoke_embed(record, payload);
        }
        let (tx, stream) = ChunkStream::channel();
        let requested = payload
            .as_object()
            .and_then(|fields| fields.get("context_length"))
            .and_then(JsonValue::as_i64);
        let context_tokens = Self::effective_context_tokens(record, requested);
        let model = wire_model_name(record);
        let backend = Arc::clone(&self.backend);
        let client = self.client.clone();
        let record = record.clone();

        tokio::spawn(async move {
            // Race the (potentially slow) server startup against a consumer drop,
            // so cancelling mid-spawn doesn't keep a cold `llama-server` booting for
            // a result nobody will read.
            let starting = backend.base_url(&record, context_tokens, ServerMode::Chat);
            let base = tokio::select! {
                result = starting => match result {
                    Ok(base) => base,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                },
                _ = tx.closed() => return,
            };
            let body = match request_body(&model, &payload) {
                Ok(body) => body,
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };
            // Local server → no API key.
            stream_completions(&client, &base, None, body, &tx).await;
        });
        stream
    }

    /// The sampling options and `context_length` for chat and completion.
    /// Embedding honours none: the request carries only its input, and an
    /// encoder's window is the one its header declares.
    fn honored_param_keys(
        &self,
        _record: &ModelRecord,
        capability: &Capability,
    ) -> HashSet<String> {
        if *capability != Capability::chat() && *capability != Capability::complete() {
            return HashSet::new();
        }
        // Derived from the OpenAI forward set so the two can't drift, plus
        // `context_length` (consumed at server-spawn as `--ctx-size`, not in the
        // request body). The llama-specific extras (top_k/min_p/repeat_penalty) are
        // deferred with a per-request llama body.
        OPTION_KEYS
            .iter()
            .copied()
            .chain(["context_length"])
            .map(str::to_owned)
            .collect()
    }
}

/// The inputs an embed payload carries: its `input` string, or each string of
/// its `input` array.
fn embed_inputs(payload: &JsonValue) -> Result<Vec<JsonValue>, RuntimeError> {
    match payload.as_object().and_then(|fields| fields.get("input")) {
        Some(JsonValue::String(input)) => Ok(vec![JsonValue::String(input.clone())]),
        Some(JsonValue::Array(inputs)) if !inputs.is_empty() => Ok(inputs.clone()),
        _ => Err(RuntimeError::Failed(
            "embed payload must carry an input".to_owned(),
        )),
    }
}

/// The inputs of each request to an embedding server launched with `window`,
/// as ranges of `inputs`: at most [`EMBEDDING_CHUNK_INPUTS`] of them, carrying
/// at most [`EMBEDDING_CHUNK_BYTES_PER_TOKEN`] bytes of text per token of the
/// window, apart from an input longer than that, which goes alone.
fn chunked(inputs: &[JsonValue], window: i64) -> Vec<Range<usize>> {
    let budget = usize::try_from(window)
        .unwrap_or(0)
        .saturating_mul(EMBEDDING_CHUNK_BYTES_PER_TOKEN);
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut bytes = 0usize;
    for (index, input) in inputs.iter().enumerate() {
        let size = input.as_str().map_or(0, str::len);
        let full = index - start == EMBEDDING_CHUNK_INPUTS
            || (index > start && bytes.saturating_add(size) > budget);
        if full {
            chunks.push(start..index);
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(size);
    }
    if start < inputs.len() {
        chunks.push(start..inputs.len());
    }
    chunks
}

/// What `work` gives, or `None` once the consumer behind `tx` has gone. The
/// work is dropped then, which closes its connection to llama-server, and
/// llama-server cancels what it had not yet started of it, so an abandoned
/// request frees the server at once rather than after the rest of its chunk.
async fn unless_abandoned<T>(
    tx: &mpsc::UnboundedSender<Result<CapabilityChunk, RuntimeError>>,
    work: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        done = work => Some(done),
        () = tx.closed() => None,
    }
}

/// A running embedding server and the model it serves.
struct EmbeddingServer<'a> {
    client: &'a reqwest::Client,
    base: &'a str,
    model: &'a str,
    /// The model's name, for a message about the model itself.
    name: &'a str,
    window: i64,
}

impl EmbeddingServer<'_> {
    /// Embed `inputs` in one request.
    async fn embed(&self, inputs: &[JsonValue]) -> Result<(Vec<Vec<f64>>, Option<i64>), Refused> {
        let body = embeddings_body(self.model, inputs).map_err(Refused::Failed)?;
        post_embeddings(self.client, self.base, body, "llama-server")
            .await
            .map_err(|failure| refused(failure, self.window, self.name))
    }

    /// The rejection naming which input of `chunk` is too long, `first` being
    /// the request's index of the chunk's first input. The server says how
    /// long, not which, so each input is embedded alone until one is refused;
    /// `too_long`, unnamed, stands when none is.
    async fn locate(&self, chunk: &[JsonValue], first: usize, too_long: TooLong) -> RuntimeError {
        for (offset, input) in chunk.iter().enumerate() {
            match self.embed(std::slice::from_ref(input)).await {
                Ok(_) => {}
                Err(Refused::TooLong(found)) => return found.rejected(Some(first + offset)),
                Err(Refused::Failed(err)) => return err,
            }
        }
        too_long.rejected(None)
    }
}

/// Why an embedding server refused a request.
enum Refused {
    /// An input is longer than the server reads at once: the caller's to fix.
    TooLong(TooLong),
    /// Anything else, which is the server's failure and never the caller's.
    Failed(RuntimeError),
}

/// An input longer than an embedding server reads at once.
#[derive(Debug, PartialEq)]
struct TooLong {
    /// Its length in tokens, special tokens included, when the server said.
    tokens: Option<u64>,
    /// The most tokens one input may hold.
    most: i64,
}

impl TooLong {
    /// The caller's error, naming the input by its `index` in a batch.
    fn rejected(&self, index: Option<usize>) -> RuntimeError {
        let subject = match index {
            Some(index) => format!("input {index}"),
            None => "the input".to_owned(),
        };
        let most = self.most;
        RuntimeError::Rejected(match self.tokens {
            Some(tokens) => {
                format!(
                    "{subject} is {tokens} tokens, more than the {most} this model embeds at once"
                )
            }
            None => format!("{subject} is more than the {most} tokens this model embeds at once"),
        })
    }
}

/// What a failed embeddings request to `name`'s server, launched with
/// `window`, means. Only an input longer than the server reads at once is the
/// caller's, and it fails again unchanged: an encoder's 500 "input (N tokens)
/// is too large to process", which allows up to the window, or a decoder's
/// 400 `exceed_context_size_error`, which allows one token less, since
/// llama-server refuses a decoder input that fills its whole context.
/// llama-server's advice to grow the batch or the context is for whoever
/// launched it, so the message is rewritten. Anything else it answers, a
/// model that pools nothing included, is the server's failure.
fn refused(failure: EmbeddingsFailure, window: i64, name: &str) -> Refused {
    let (status, body) = match failure {
        EmbeddingsFailure::Runtime(err) => return Refused::Failed(err),
        EmbeddingsFailure::Answered(status, body) => (status, body),
    };
    let message = body.message.as_deref().unwrap_or_default();
    match status {
        500 => {
            if let Some(tokens) = too_long_input(message, "input (", "is too large to process") {
                return Refused::TooLong(TooLong {
                    tokens,
                    most: window,
                });
            }
        }
        400 if body.kind.as_deref() == Some("exceed_context_size_error") => {
            return Refused::TooLong(TooLong {
                tokens: too_long_input(message, "request (", "exceeds the available context size")
                    .flatten(),
                most: window - 1,
            });
        }
        _ => {}
    }
    if message.starts_with("Pooling type 'none'") {
        return Refused::Failed(RuntimeError::Failed(format!(
            "{name} pools nothing into one vector, so it cannot embed"
        )));
    }
    Refused::Failed(RuntimeError::Failed(
        body.message
            .unwrap_or_else(|| format!("llama-server answered with HTTP {status}")),
    ))
}

/// Whether `message` reads `{lead}N tokens) {verdict}...`: `Some` with the
/// token count `N` when it parses, and `None` when the message is another one.
fn too_long_input(message: &str, lead: &str, verdict: &str) -> Option<Option<u64>> {
    let (count, rest) = message.strip_prefix(lead)?.split_once(" tokens)")?;
    rest.trim_start()
        .starts_with(verdict)
        .then(|| count.parse().ok())
}

/// The wire model name `llama-server` is asked for. The server serves whatever
/// GGUF it loaded regardless, so this is only a label; a repo id is preferred over
/// the display name.
fn wire_model_name(record: &ModelRecord) -> String {
    record
        .source
        .repo
        .clone()
        .unwrap_or_else(|| record.name.clone())
}

/// The GGUF path a `llama-server` is launched with: the primary weight file when
/// one dominates, else the source path. Returned as stored (no `~` expansion —
/// discovery passes absolute paths).
pub(crate) fn model_gguf_path(record: &ModelRecord) -> &str {
    record
        .primary_weight_path
        .as_deref()
        .unwrap_or(&record.source.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::records::{Modality, ModelSource, SourceKind};

    fn record() -> ModelRecord {
        ModelRecord::new(
            "m",
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::file(), "/w.gguf"),
        )
    }

    #[test]
    fn wire_model_name_prefers_the_repo() {
        let mut rec = record();
        assert_eq!(wire_model_name(&rec), "m");
        rec.source.repo = Some("org/model".to_owned());
        assert_eq!(wire_model_name(&rec), "org/model");
    }

    fn answered(status: u16, message: &str, kind: &str) -> EmbeddingsFailure {
        EmbeddingsFailure::Answered(
            status,
            crate::adapters::openai::ErrorBody {
                message: Some(message.to_owned()),
                kind: Some(kind.to_owned()),
            },
        )
    }

    fn too_long(refused: Refused) -> TooLong {
        match refused {
            Refused::TooLong(too_long) => too_long,
            Refused::Failed(err) => panic!("expected the caller's input, got {err:?}"),
        }
    }

    fn message(err: RuntimeError) -> String {
        match err {
            RuntimeError::Rejected(message) => message,
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn an_encoders_too_large_input_is_the_callers_and_names_the_window() {
        let failure = answered(
            500,
            "input (2102 tokens) is too large to process. increase the physical batch size (current batch size: 2048)",
            "server_error",
        );
        let found = too_long(refused(failure, 2048, "nomic"));
        assert_eq!(
            message(found.rejected(None)),
            "the input is 2102 tokens, more than the 2048 this model embeds at once"
        );
        assert_eq!(
            message(found.rejected(Some(9000))),
            "input 9000 is 2102 tokens, more than the 2048 this model embeds at once"
        );
    }

    #[test]
    fn a_decoders_exceeded_context_names_the_one_token_less_it_takes() {
        let failure = answered(
            400,
            "request (8192 tokens) exceeds the available context size (8192 tokens), try increasing it",
            "exceed_context_size_error",
        );
        let found = too_long(refused(failure, 8192, "qwen3"));
        assert_eq!(
            found,
            TooLong {
                tokens: Some(8192),
                most: 8191,
            }
        );
        let text = message(found.rejected(None));
        assert_eq!(
            text,
            "the input is 8192 tokens, more than the 8191 this model embeds at once"
        );
        assert!(!text.contains("increas"), "{text}");
        let reworded = answered(400, "too long", "exceed_context_size_error");
        assert_eq!(
            message(too_long(refused(reworded, 8192, "qwen3")).rejected(Some(2))),
            "input 2 is more than the 8191 tokens this model embeds at once"
        );
    }

    #[test]
    fn a_model_side_or_unrecognized_failure_is_never_the_callers() {
        let too_large = "input (2102 tokens) is too large to process";
        for failure in [
            answered(502, too_large, "server_error"),
            answered(503, too_large, "server_error"),
            answered(400, too_large, "invalid_request_error"),
            answered(500, "the model crashed", "server_error"),
        ] {
            assert!(
                matches!(
                    refused(failure, 2048, "m"),
                    Refused::Failed(RuntimeError::Failed(_))
                ),
                "a failure became the caller's"
            );
        }
        let unreachable = EmbeddingsFailure::Runtime(RuntimeError::Unavailable("down".to_owned()));
        assert!(matches!(
            refused(unreachable, 2048, "m"),
            Refused::Failed(RuntimeError::Unavailable(_))
        ));
    }

    #[test]
    fn a_model_that_pools_nothing_is_named_without_llama_servers_advice() {
        let failure = answered(
            400,
            "Pooling type 'none' is not OAI compatible. Please use a different pooling type",
            "invalid_request_error",
        );
        match refused(failure, 512, "t5-base-encoder") {
            Refused::Failed(RuntimeError::Failed(text)) => assert_eq!(
                text,
                "t5-base-encoder pools nothing into one vector, so it cannot embed"
            ),
            Refused::Failed(other) => panic!("expected Failed, got {other:?}"),
            Refused::TooLong(found) => panic!("expected a server failure, got {found:?}"),
        }
    }

    #[test]
    fn each_record_is_served_in_the_one_mode_its_claims_name() {
        let with = |capabilities: Vec<Capability>| {
            let mut rec = record();
            rec.capabilities = capabilities;
            ServerMode::of(&rec)
        };
        assert_eq!(with(vec![Capability::embed()]), Some(ServerMode::Embedding));
        assert_eq!(
            with(vec![Capability::chat(), Capability::tools()]),
            Some(ServerMode::Chat)
        );
        assert_eq!(with(vec![Capability::complete()]), Some(ServerMode::Chat));
        assert_eq!(with(Vec::new()), Some(ServerMode::Chat));
        assert_eq!(with(vec![Capability::judge()]), None);
        assert!(!ServerMode::Chat.serves(&Capability::embed()));
        assert!(!ServerMode::Embedding.serves(&Capability::chat()));
        assert!(ServerMode::Embedding.serves(&Capability::embed()));
    }

    #[test]
    fn a_batch_is_chunked_by_count_and_by_about_one_window_of_text() {
        let texts = |sizes: &[usize]| -> Vec<JsonValue> {
            sizes
                .iter()
                .map(|size| JsonValue::String("x".repeat(*size)))
                .collect()
        };
        // Short inputs: 64 to a chunk.
        let short = texts(&[10; 130]);
        assert_eq!(chunked(&short, 2048), [0..64, 64..128, 128..130]);
        // A 2048-token window carries 8 KiB of text.
        let long = texts(&[3000, 3000, 3000, 100, 9000, 10]);
        assert_eq!(chunked(&long, 2048), [0..2, 2..4, 4..5, 5..6]);
        // One input over the budget goes alone, first or not.
        let alone = chunked(&texts(&[20000]), 2048);
        assert_eq!((alone.len(), alone.first()), (1, Some(&(0..1))));
        assert_eq!(chunked(&texts(&[5, 20000, 5]), 2048), [0..1, 1..2, 2..3]);
        // Exactly the budget fits.
        assert_eq!(chunked(&texts(&[4096, 4096, 1]), 2048), [0..2, 2..3]);
    }

    #[test]
    fn only_a_decoder_that_pools_by_the_last_token_takes_one_token_less() {
        assert_eq!(EmbedderKind::Encoder.most_tokens(2048), 2048);
        let mean = EmbedderKind::Decoder { pools_last: false };
        assert_eq!(mean.most_tokens(8192), 8192);
        let last = EmbedderKind::Decoder { pools_last: true };
        assert_eq!(last.most_tokens(8192), 8191);
    }

    #[test]
    fn an_embedding_window_is_capped() {
        let mut rec = record();
        rec.capabilities = vec![Capability::embed()];
        rec.context_length = Some(32768);
        assert_eq!(LlamaServerAdapter::embedding_context_tokens(&rec), 8192);
        rec.context_length = Some(2048);
        assert_eq!(LlamaServerAdapter::embedding_context_tokens(&rec), 2048);
    }

    #[test]
    fn effective_context_treats_nonpositive_declared_length_as_the_default() {
        let mut rec = record();
        rec.context_length = Some(0);
        assert_eq!(
            LlamaServerAdapter::effective_context_tokens(&rec, None),
            4096
        );
        rec.context_length = Some(-5);
        assert_eq!(
            LlamaServerAdapter::effective_context_tokens(&rec, None),
            4096
        );
    }
}
