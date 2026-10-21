//! The local llama.cpp adapter, served through `llama-server` rather than an
//! in-process FFI engine. `llama-server` speaks the OpenAI API, so this adapter
//! ensures a server is running for the model's GGUF, then proxies the request
//! through the shared OpenAI streaming path. A GGUF that only embeds is served
//! from its own embedding-mode server over `/v1/embeddings`, and a decision
//! model (clef, laya, kev, openjev, lev) from a decision server over
//! `/v1/systemone`; no model is asked for what its mode does not serve.
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

use kernel::records::{
    BidPreference, Capability, JsonValue, ModelRecord, RunTier, RuntimeId, SourceKind,
};
use kernel::resolution::{
    GgufPooling, IdentifiedModel, ModelFormat, RuntimeBid, gguf_facts, launch_gguf_path,
    projector_for,
};

use kernel::capabilities::{CapabilityChunk, GenerationStats};
use tokio::sync::mpsc;

use super::llama_decision;
use super::llama_pool::exited_during_startup;
use super::openai::{
    OPTION_KEYS, PostFailure, embeddings_body, post_embeddings, request_body, stream_completions,
    tokens_before,
};
use super::{ChunkStream, RuntimeAdapter, RuntimeError};

const DEFAULT_CONTEXT: i64 = 4096;
const MAX_DEFAULT_CONTEXT: i64 = 32768;
const MIN_CONTEXT: i64 = 512;

/// The window a chat server starts at when the memory there seems too small
/// for any: worth a try, and a server that cannot take it says it ran out of
/// memory.
const SMALL_WINDOW: i64 = 2048;

/// Reads the memory a llama-server can take as it starts, right now.
pub type LaunchMemory = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

/// The widest window an embedding server is launched with. An encoder reads a
/// whole input in one physical batch, so the batch is the window, and both
/// cost memory whatever the input: Qwen3-Embedding-0.6B declares 32768 and
/// idles near 4.4 GB there against 1.6 GB at 8192, and one 8192-token input
/// through a bge-m3-sized encoder peaks near 9 GB. 8192 is the window bge-m3
/// and jina v2 are trained with, so the long-input encoders lose nothing.
const MAX_EMBEDDING_CONTEXT: i64 = 8192;

/// The architectures llama.cpp keeps no KV cache for
/// (`llama_model::create_memory` returns none): encoders that embed, or that
/// score typed questions as `modern-bert` (laya) does. The encoders
/// `llama-embed` and `t5encoder` get a cache like any decoder.
const MEMORYLESS_ARCHITECTURES: [&str; 10] = [
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

/// The widest window a decision server is launched with. llama.cpp reads a
/// laya, kev or clef prompt in one physical batch, so the batch is the window
/// and costs compute memory whatever the prompt; clef's reference code encodes
/// at most 16384 tokens, and the qwen35 decision models declare 262144, far
/// past what one batch should hold.
const MAX_DECISION_CONTEXT: i64 = 16384;

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
    /// Typed questions over `/v1/systemone`, for a decision model, started
    /// with a batch that holds the whole prompt.
    Decision,
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

/// Whether llama.cpp keeps a KV cache for the GGUF at `path`, as its header
/// names the architecture. A header that cannot be read is taken to keep one,
/// which launches safely.
pub(crate) fn keeps_kv_cache(path: &str) -> bool {
    gguf_facts(Path::new(path))
        .and_then(|facts| facts.architecture)
        .is_none_or(|name| !MEMORYLESS_ARCHITECTURES.contains(&name.as_str()))
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
        if architecture.is_some_and(|name| MEMORYLESS_ARCHITECTURES.contains(&name)) {
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
    /// The mode `record` is served in: chat for one that claims chat or
    /// completion, or nothing at all (a pin over an unidentified file);
    /// decision for one that claims `judge` and neither; embedding for one
    /// that claims `embed` and none of those; and none for a record whose
    /// claims no mode answers.
    pub(crate) fn of(record: &ModelRecord) -> Option<Self> {
        let claims = |capability: Capability| record.capabilities.contains(&capability);
        if claims(Capability::chat()) || claims(Capability::complete()) {
            Some(Self::Chat)
        } else if claims(Capability::judge()) {
            Some(Self::Decision)
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
            Self::Decision => *capability == Capability::judge(),
        }
    }

    /// Whether a server in this mode for `record` reads images: a chat or
    /// decision server for a record that claims sight is launched with its
    /// projector. An embedding server never reads one.
    pub(crate) fn reads_images(self, record: &ModelRecord) -> bool {
        self != Self::Embedding && record.capabilities.contains(&Capability::see())
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

    /// The window of the chat server running for `record`, when one is and
    /// is not being replaced.
    fn resident_window(&self, _record: &ModelRecord) -> Option<i64> {
        None
    }
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
    /// The window each Ollama model's GGUF declares, by path, read the same
    /// way: the record carries Ollama's `num_ctx` instead.
    declared: Mutex<HashMap<String, (FileSignature, Option<i64>)>>,
    /// The KV cache a token takes in each GGUF, by path, read the same way.
    kv_per_token: Mutex<HashMap<String, (FileSignature, Option<u64>)>>,
    /// The memory a server can take as it starts, which the default window
    /// of a chat server is kept within; unknown when unset.
    memory: Option<LaunchMemory>,
}

impl LlamaServerAdapter {
    /// An adapter over `backend`.
    pub fn new(backend: Arc<dyn LlamaBackend>) -> Self {
        Self {
            id: RuntimeId::llama_cpp(),
            backend,
            client: reqwest::Client::new(),
            kinds: Mutex::new(HashMap::new()),
            declared: Mutex::new(HashMap::new()),
            kv_per_token: Mutex::new(HashMap::new()),
            memory: None,
        }
    }

    /// Keep the window a chat server starts at by default within what
    /// `memory` says a server can take as it starts.
    pub fn with_memory(mut self, memory: LaunchMemory) -> Self {
        self.memory = Some(memory);
        self
    }

    /// The kind of the embedder GGUF at `path`, read again only when the file
    /// changed since it was last read.
    fn embedder_kind(&self, path: &str) -> EmbedderKind {
        read_cached(&self.kinds, path, EmbedderKind::of)
    }

    /// The largest window `record` takes here, and the one a request that
    /// names none starts at, if the model sets one. A model in Ollama's store
    /// declares its window in its GGUF, and its record carries Ollama's
    /// `num_ctx`, the window Ollama would open it with.
    fn window_bounds(&self, record: &ModelRecord) -> (Option<i64>, Option<i64>) {
        if record.source.kind != SourceKind::ollama() {
            return (record.context_length, None);
        }
        let declared = read_cached(&self.declared, model_gguf_path(record), |path| {
            gguf_facts(Path::new(path)).and_then(|facts| facts.context_length)
        });
        (declared.or(record.context_length), record.context_length)
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
        let window = self.embedding_context_tokens(record);
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
    fn embedding_context_tokens(&self, record: &ModelRecord) -> i64 {
        let (limit, _) = self.window_bounds(record);
        clamped_window(limit, None).min(MAX_EMBEDDING_CONTEXT)
    }

    /// The window a chat server for `record` is launched with for a request
    /// asking for `requested`. One asked for is used as asked; the default is
    /// kept to what fits in memory beside the weights, so a model that
    /// declares a long window still starts on a small machine.
    fn chat_context_tokens(&self, record: &ModelRecord, requested: Option<i64>) -> i64 {
        let (limit, default) = self.window_bounds(record);
        if requested.is_some() {
            return clamped_window(limit, requested);
        }
        let window = clamped_window(limit, default);
        // A server already running took its memory when it started; judging
        // again now would count its weights twice.
        if let Some(resident) = self.backend.resident_window(record) {
            return window.min(resident);
        }
        match self.window_that_fits(record) {
            Some(0) => window.min(SMALL_WINDOW),
            Some(fits) => window.min(fits),
            None => window,
        }
    }

    /// The widest window whose KV cache fits in the memory a server can take
    /// now, beside what serving `record` loads (every shard of a split model
    /// and the projector it is given), when both are known.
    fn window_that_fits(&self, record: &ModelRecord) -> Option<i64> {
        let memory = self.memory.as_ref()?();
        let gguf = model_gguf_path(record);
        let kv = read_cached(&self.kv_per_token, gguf, |path| {
            gguf_facts(Path::new(path)).and_then(|facts| facts.kv_bytes_per_token)
        })?;
        let size = |path: &Path| std::fs::metadata(path).map(|meta| meta.len()).ok();
        // Measured now, as the server will load it: every shard of a split
        // model and the projector it is given, which it is only when the
        // model sees. Never less than what the shelf measured serving it
        // takes: a hub repo's split model is named by a blob's hash, so its
        // other shards are not found beside it.
        let projector = if ServerMode::Chat.reads_images(record) {
            projector_for(record)
                .and_then(|projector| size(&projector))
                .unwrap_or(0)
        } else {
            0
        };
        let measured = gguf_set_bytes(&launch_gguf_path(record))? + projector;
        let serving = record
            .serving_bytes
            .and_then(|bytes| u64::try_from(bytes).ok())
            .unwrap_or(0);
        let loaded = measured.max(serving);
        kernel::machine::window_that_fits(memory?, loaded, kv)
    }

    /// Put the typed question `payload` carries to a decision server and send
    /// back its System One envelope whole, as one text chunk. The question
    /// goes as the caller wrote it, so no key moves. A consumer that goes away
    /// drops the request in flight.
    fn invoke_decision(&self, record: &ModelRecord, payload: &JsonValue) -> ChunkStream {
        let (tx, stream) = ChunkStream::channel();
        let question = match llama_decision::question_text(payload) {
            Ok(question) => question,
            Err(err) => {
                let _ = tx.send(Err(err));
                return stream;
            }
        };
        let window = self.decision_context_tokens(record);
        let backend = Arc::clone(&self.backend);
        let client = self.client.clone();
        let record = record.clone();

        tokio::spawn(async move {
            let name = record.name.as_str();
            let starting = backend.base_url(&record, window, ServerMode::Decision);
            let base = tokio::select! {
                result = starting => match result {
                    Ok(base) => base,
                    // An older llama.cpp dies on a decision architecture it
                    // does not know, and its stderr is not kept, so the likely
                    // reason is offered beside what is known.
                    Err(err) if exited_during_startup(&err) => {
                        let _ = tx.send(Err(RuntimeError::Unavailable(format!(
                            "{err}. {}; an older one exits loading it",
                            llama_decision::upgrade_hint(name)
                        ))));
                        return;
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return;
                    }
                },
                _ = tx.closed() => return,
            };
            let asked = llama_decision::ask(&client, &base, &question, "llama-server");
            let Some(asked) = unless_abandoned(&tx, asked).await else {
                return;
            };
            let answer = match asked {
                Ok(answer) => answer,
                Err(PostFailure::Runtime(err)) => {
                    let _ = tx.send(Err(err));
                    return;
                }
                Err(PostFailure::Answered(status, body)) => {
                    let _ = tx.send(Err(llama_decision::refusal(
                        status, body, &question, window, name,
                    )));
                    return;
                }
            };
            if tx.send(Ok(CapabilityChunk::Text(answer.envelope))).is_err() {
                return;
            }
            let stats = GenerationStats {
                prompt_tokens: answer.input_tokens,
                ..Default::default()
            };
            let _ = tx.send(Ok(CapabilityChunk::Done(Some(stats))));
        });
        stream
    }

    /// The window a decision server for `record` is launched with: its
    /// declared window, capped at [`MAX_DECISION_CONTEXT`]. A judge takes no
    /// options, so nothing a request says changes it.
    pub(crate) fn decision_context_tokens(&self, record: &ModelRecord) -> i64 {
        let (limit, _) = self.window_bounds(record);
        clamped_window(limit, None).min(MAX_DECISION_CONTEXT)
    }

    /// The effective context window: the requested size (or a capped default)
    /// clamped into `[min(512, base), base]`, where `base` is the model's declared
    /// context length or 4096.
    pub fn effective_context_tokens(record: &ModelRecord, requested: Option<i64>) -> i64 {
        clamped_window(record.context_length, requested)
    }
}

/// The bytes of the GGUF at `path` and, when it is one shard of a split model
/// (`<base>-00001-of-00003.gguf`), of every shard of it beside it.
fn gguf_set_bytes(path: &Path) -> Option<u64> {
    let size = |path: &Path| std::fs::metadata(path).map(|meta| meta.len()).ok();
    let Some(shard) = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(kernel::discovery::parse)
    else {
        return size(path);
    };
    let entries = std::fs::read_dir(path.parent()?).ok()?;
    Some(
        entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(kernel::discovery::parse)
                    .is_some_and(|other| other.base == shard.base && other.total == shard.total)
            })
            .filter_map(|entry| size(&entry.path()))
            .sum(),
    )
}

/// `payload` carrying the stop sequences saved with `record` (an Ollama
/// model's params, which Ollama applies itself) when it names none of its own.
fn with_saved_stops(record: &ModelRecord, mut payload: JsonValue) -> JsonValue {
    if let (Some(stops), JsonValue::Object(fields)) = (&record.stop_tokens, &mut payload)
        && !stops.is_empty()
        && !fields.contains_key("stop")
    {
        let stops = stops.iter().cloned().map(JsonValue::String).collect();
        fields.insert("stop".to_owned(), JsonValue::Array(stops));
    }
    payload
}

/// `requested` (or a capped default) clamped into `[min(512, base), base]`,
/// where `base` is `limit` or 4096.
fn clamped_window(limit: Option<i64>, requested: Option<i64>) -> i64 {
    let base = limit
        .filter(|length| *length > 0)
        .unwrap_or(DEFAULT_CONTEXT);
    let capped_default = base.min(MAX_DEFAULT_CONTEXT);
    let lower = base.min(MIN_CONTEXT);
    requested.unwrap_or(capped_default).max(lower).min(base)
}

/// What `read` makes of the file at `path`, from `cache` while the file's
/// modification time and size are those it was read at.
fn read_cached<T: Copy>(
    cache: &Mutex<HashMap<String, (FileSignature, T)>>,
    path: &str,
    read: impl Fn(&str) -> T,
) -> T {
    let Some(signature) = std::fs::metadata(path)
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())))
    else {
        return read(path);
    };
    if let Ok(cached) = cache.lock()
        && let Some((seen, value)) = cached.get(path)
        && *seen == signature
    {
        return *value;
    }
    let value = read(path);
    if let Ok(mut cached) = cache.lock() {
        cached.insert(path.to_owned(), (signature, value));
    }
    value
}

impl RuntimeAdapter for LlamaServerAdapter {
    fn id(&self) -> &RuntimeId {
        &self.id
    }

    fn wires_tools(&self) -> bool {
        true
    }

    /// The capabilities of the one mode `record` is served in, and sight for a
    /// record that claims it, since its chat or decision server is launched
    /// with its projector. A model in Ollama's store claims sight by what
    /// Ollama reads, so it sees here only when its projector layer is one
    /// llama.cpp loads.
    fn can_serve(&self, record: &ModelRecord, capability: &Capability) -> bool {
        if record.runtime.id.as_ref() != Some(&self.id) {
            return false;
        }
        let Some(mode) = ServerMode::of(record) else {
            return false;
        };
        if *capability == Capability::see() {
            return mode.reads_images(record)
                && (record.source.kind != SourceKind::ollama() || projector_for(record).is_some());
        }
        mode.serves(capability)
    }

    /// A GGUF that chats, one that embeds, or a decision model that judges. A
    /// GGUF that claims none of those (a projector, a vocoder, a model whose
    /// header pools nothing or ranks) draws no bid, since no server mode would
    /// answer for it. A model in Ollama's store is a GGUF too, bid on behind
    /// Ollama when llama.cpp can load it: no encoder packed into the weights,
    /// and, to chat, a template of the GGUF's own.
    fn bid(&self, _record: &ModelRecord, identified: &IdentifiedModel) -> Option<RuntimeBid> {
        let claims = |capability: Capability| identified.capabilities.contains(&capability);
        if identified.format == ModelFormat::OllamaStore {
            let loadable = identified.bundled_encoders == Some(false)
                && (!claims(Capability::chat()) || identified.gguf_chat_template == Some(true));
            let servable = claims(Capability::chat()) || claims(Capability::embed());
            return (loadable && servable)
                .then(|| RuntimeBid::new(RunTier::Native, BidPreference::LLAMA_CPP_OLLAMA_STORE));
        }
        let servable = claims(Capability::chat())
            || claims(Capability::embed())
            || claims(Capability::judge());
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
                .most_tokens(self.embedding_context_tokens(record)),
            Some(ServerMode::Decision) => self.decision_context_tokens(record),
            _ => self.chat_context_tokens(record, requested),
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
        if capability == Capability::judge() {
            return self.invoke_decision(record, &payload);
        }
        let (tx, stream) = ChunkStream::channel();
        let payload = with_saved_stops(record, payload);
        let requested = payload
            .as_object()
            .and_then(|fields| fields.get("context_length"))
            .and_then(JsonValue::as_i64);
        let context_tokens = self.chat_context_tokens(record, requested);
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
            let sees = ServerMode::Chat.reads_images(&record);
            let body = match request_body(&model, &payload, sees) {
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
fn refused(failure: PostFailure, window: i64, name: &str) -> Refused {
    let (status, body) = match failure {
        PostFailure::Runtime(err) => return Refused::Failed(err),
        PostFailure::Answered(status, body) => (status, body),
    };
    let message = body.message.as_deref().unwrap_or_default();
    match status {
        500 => {
            if let Some(tokens) = tokens_before(message, "input (", "is too large to process") {
                return Refused::TooLong(TooLong {
                    tokens,
                    most: window,
                });
            }
        }
        400 if body.kind.as_deref() == Some("exceed_context_size_error") => {
            return Refused::TooLong(TooLong {
                tokens: tokens_before(message, "request (", "exceeds the available context size")
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

    /// A backend no unit test here asks for a server.
    struct NoServer;

    impl LlamaBackend for NoServer {
        fn base_url(&self, _: &ModelRecord, _: i64, _: ServerMode) -> BackendFuture {
            Box::pin(async { Err(RuntimeError::Unavailable("no server".to_owned())) })
        }
    }

    fn adapter() -> LlamaServerAdapter {
        LlamaServerAdapter::new(Arc::new(NoServer))
    }

    #[test]
    fn wire_model_name_prefers_the_repo() {
        let mut rec = record();
        assert_eq!(wire_model_name(&rec), "m");
        rec.source.repo = Some("org/model".to_owned());
        assert_eq!(wire_model_name(&rec), "org/model");
    }

    fn answered(status: u16, message: &str, kind: &str) -> PostFailure {
        PostFailure::Answered(
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
        let unreachable = PostFailure::Runtime(RuntimeError::Unavailable("down".to_owned()));
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
        assert_eq!(with(vec![Capability::judge()]), Some(ServerMode::Decision));
        assert_eq!(
            with(vec![Capability::judge(), Capability::see()]),
            Some(ServerMode::Decision)
        );
        assert_eq!(
            with(vec![Capability::chat(), Capability::judge()]),
            Some(ServerMode::Chat)
        );
        assert_eq!(with(vec![Capability::see()]), None);
        assert!(!ServerMode::Chat.serves(&Capability::embed()));
        assert!(!ServerMode::Chat.serves(&Capability::judge()));
        assert!(!ServerMode::Embedding.serves(&Capability::chat()));
        assert!(!ServerMode::Embedding.serves(&Capability::judge()));
        assert!(ServerMode::Embedding.serves(&Capability::embed()));
        assert!(ServerMode::Decision.serves(&Capability::judge()));
        for other in [
            Capability::chat(),
            Capability::complete(),
            Capability::embed(),
            Capability::see(),
        ] {
            assert!(!ServerMode::Decision.serves(&other), "{other:?}");
        }
    }

    #[test]
    fn a_decision_window_is_the_declared_one_capped_at_16384_whatever_is_asked() {
        let window = |declared: Option<i64>| {
            let mut rec = record();
            rec.capabilities = vec![Capability::judge()];
            rec.context_length = declared;
            adapter().decision_context_tokens(&rec)
        };
        assert_eq!(window(Some(262_144)), 16384);
        assert_eq!(window(Some(8192)), 8192);
        assert_eq!(window(None), 4096);
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
    fn saved_stop_sequences_go_with_a_request_that_names_none() {
        let mut rec = record();
        rec.stop_tokens = Some(vec!["<|end|>".to_owned()]);
        let payload = |stop: Option<&str>| {
            let mut fields = std::collections::BTreeMap::new();
            if let Some(stop) = stop {
                fields.insert("stop".to_owned(), JsonValue::String(stop.to_owned()));
            }
            JsonValue::Object(fields.into_iter().collect())
        };
        let stops = |payload: &JsonValue| payload.as_object().and_then(|f| f.get("stop")).cloned();

        let seeded = with_saved_stops(&rec, payload(None));
        assert_eq!(
            stops(&seeded),
            Some(JsonValue::Array(vec![JsonValue::String(
                "<|end|>".to_owned()
            )]))
        );
        let own = with_saved_stops(&rec, payload(Some("###")));
        assert_eq!(stops(&own), Some(JsonValue::String("###".to_owned())));
        rec.stop_tokens = None;
        assert_eq!(stops(&with_saved_stops(&rec, payload(None))), None);
    }

    #[test]
    fn an_embedding_window_is_capped() {
        let mut rec = record();
        rec.capabilities = vec![Capability::embed()];
        rec.context_length = Some(32768);
        assert_eq!(adapter().embedding_context_tokens(&rec), 8192);
        rec.context_length = Some(2048);
        assert_eq!(adapter().embedding_context_tokens(&rec), 2048);
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

    /// A GGUF header with `kvs`, each a key and a `u32`, and no tensors.
    fn header(kvs: &[(&str, u32)]) -> Vec<u8> {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(kvs.len() as u64 + 1).to_le_bytes());
        let string = |bytes: &mut Vec<u8>, text: &str| {
            bytes.extend_from_slice(&(text.len() as u64).to_le_bytes());
            bytes.extend_from_slice(text.as_bytes());
        };
        string(&mut bytes, "general.architecture");
        bytes.extend_from_slice(&8u32.to_le_bytes());
        string(&mut bytes, "llama");
        for (key, value) in kvs {
            string(&mut bytes, key);
            bytes.extend_from_slice(&4u32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn the_default_window_is_kept_to_what_fits_in_memory() {
        let dir = std::env::temp_dir().join(format!("hedos-window-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mistral.gguf");
        // Mistral 7B's shape: 128 KiB of KV cache a token.
        std::fs::write(
            &path,
            header(&[
                ("llama.block_count", 32),
                ("llama.embedding_length", 4096),
                ("llama.attention.head_count", 32),
                ("llama.attention.head_count_kv", 8),
            ]),
        )
        .unwrap();
        let mut rec = record();
        rec.source.path = path.display().to_string();
        rec.context_length = Some(32768);
        let gib = 1u64 << 30;
        let with = |memory: Option<u64>| adapter().with_memory(Arc::new(move || memory));

        // 2 GiB (and a MiB for the header itself) leaves 1 GiB beside the
        // headroom: 8192 tokens.
        let two = 2 * gib + (1 << 20);
        assert_eq!(with(Some(two)).chat_context_tokens(&rec, None), 8192);
        assert_eq!(with(Some(64 * gib)).chat_context_tokens(&rec, None), 32768);
        assert_eq!(with(None).chat_context_tokens(&rec, None), 32768);
        assert_eq!(adapter().chat_context_tokens(&rec, None), 32768);
        // Too little for any window: the small one is still tried. A small
        // window that fits is kept, not raised to it.
        assert_eq!(
            with(Some(gib)).chat_context_tokens(&rec, None),
            SMALL_WINDOW
        );
        let tight = gib + (129 << 20);
        assert_eq!(with(Some(tight)).chat_context_tokens(&rec, None), 1024);
        // A server already running keeps its window.
        struct Resident;
        impl LlamaBackend for Resident {
            fn base_url(&self, _: &ModelRecord, _: i64, _: ServerMode) -> BackendFuture {
                Box::pin(async { Err(RuntimeError::Unavailable("no server".to_owned())) })
            }
            fn resident_window(&self, _: &ModelRecord) -> Option<i64> {
                Some(16384)
            }
        }
        // A split model counts every shard beside the one the record names.
        let shards = dir.join("shards");
        std::fs::create_dir_all(&shards).unwrap();
        let first = shards.join("m-00001-of-00002.gguf");
        std::fs::write(&first, std::fs::read(&path).unwrap()).unwrap();
        let second = std::fs::File::create(shards.join("m-00002-of-00002.gguf")).unwrap();
        second.set_len(512 << 20).unwrap();
        let mut split = rec.clone();
        split.source.path = first.display().to_string();
        let roomy = 2 * gib + (513 << 20);
        assert_eq!(with(Some(roomy)).chat_context_tokens(&split, None), 8192);
        // A repo's other quantizations (its footprint) count for nothing;
        // what the shelf measured serving takes is a floor, for a split model
        // whose other shards cannot be found by name.
        let mut repo = rec.clone();
        repo.footprint_bytes = Some(40 << 30);
        assert_eq!(with(Some(two)).chat_context_tokens(&repo, None), 8192);
        repo.serving_bytes = Some(512 << 20);
        assert_eq!(with(Some(roomy)).chat_context_tokens(&repo, None), 8192);
        assert_eq!(with(Some(two)).chat_context_tokens(&repo, None), 4096);
        let resident =
            LlamaServerAdapter::new(Arc::new(Resident)).with_memory(Arc::new(move || Some(gib)));
        assert_eq!(resident.chat_context_tokens(&rec, None), 16384);
        // A window asked for is used as asked.
        assert_eq!(
            with(Some(2 * gib)).chat_context_tokens(&rec, Some(16384)),
            16384
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
