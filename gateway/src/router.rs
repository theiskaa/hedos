//! The router: the route table and the dispatch flow — authenticate, match the
//! route, cap concurrent inference, run the handler, audit, and render an error
//! if the handler failed before it started responding.

use std::sync::Arc;

use crate::admission::GatewayCounter;
use crate::audit::{Auditing, GatewayAuditEntry};
use crate::auth::Authenticating;
use crate::defaults::SATURATED_RETRY_AFTER_SECONDS;
use crate::error::{GatewayError, GatewayErrorKind};
use crate::handlers::GatewayHandling;
use crate::handlers::chat::{OllamaChatHandler, OpenAIChatHandler};
use crate::handlers::embeddings::{OllamaEmbedHandler, OpenAIEmbeddingsHandler};
use crate::handlers::generate::{OllamaGenerateHandler, OpenAICompletionsHandler};
use crate::handlers::images::OpenAIImagesHandler;
use crate::handlers::models::{
    OllamaShowHandler, OllamaTagsHandler, OllamaVersionHandler, OpenAIModelsHandler,
};
use crate::handlers::speech::OpenAISpeechHandler;
use crate::identity::{GatewayIdentity, GatewayOutcome};
use crate::port::GatewayPort;
use crate::request::GatewayRequest;
use crate::responder::GatewayResponder;
use crate::surface::GatewaySurface;
use crate::wire::timestamp::now_unix_millis;

/// One route: the method and path it answers, the handler, and metadata.
pub struct GatewayRoute {
    /// The HTTP method (upper-cased).
    pub method: String,
    /// The path this route answers.
    pub path: String,
    /// The handler that serves it.
    pub handler: Box<dyn GatewayHandling>,
    /// Whether this route counts against the concurrent-inference cap.
    pub inference: bool,
    /// The display group (`OpenAI` / `Ollama`).
    pub group: String,
    /// A one-line summary for the endpoint catalog.
    pub summary: String,
    /// A per-route body-size override (for large uploads).
    pub max_body_bytes: Option<usize>,
}

impl GatewayRoute {
    /// A route with default (non-inference, ungrouped) metadata.
    pub fn new(method: &str, path: &str, handler: Box<dyn GatewayHandling>) -> Self {
        Self {
            method: method.to_uppercase(),
            path: path.to_owned(),
            handler,
            inference: false,
            group: String::new(),
            summary: String::new(),
            max_body_bytes: None,
        }
    }

    /// Mark this route as inference (subject to the concurrency cap).
    pub fn inference(mut self) -> Self {
        self.inference = true;
        self
    }

    /// Set the display group and summary.
    pub fn described(mut self, group: &str, summary: &str) -> Self {
        self.group = group.to_owned();
        self.summary = summary.to_owned();
        self
    }
}

/// The routes served today: chat, embeddings, completions, image, and speech on
/// both surfaces plus the model-listing and handshake endpoints. Transcription
/// lands as its handler does.
pub fn standard_routes() -> Vec<GatewayRoute> {
    vec![
        GatewayRoute::new(
            "POST",
            "/v1/chat/completions",
            Box::new(OpenAIChatHandler::default()),
        )
        .inference()
        .described("OpenAI", "Stream or complete a chat"),
        GatewayRoute::new("GET", "/v1/models", Box::new(OpenAIModelsHandler))
            .described("OpenAI", "List the models this token can reach"),
        GatewayRoute::new("POST", "/api/chat", Box::new(OllamaChatHandler::default()))
            .inference()
            .described("Ollama", "Chat over the Ollama NDJSON protocol"),
        GatewayRoute::new("GET", "/api/tags", Box::new(OllamaTagsHandler))
            .described("Ollama", "List models, Ollama-style"),
        GatewayRoute::new("GET", "/api/version", Box::new(OllamaVersionHandler))
            .described("Ollama", "Version handshake for stock clients"),
        GatewayRoute::new("POST", "/api/show", Box::new(OllamaShowHandler))
            .described("Ollama", "Model details handshake"),
        GatewayRoute::new("POST", "/v1/embeddings", Box::new(OpenAIEmbeddingsHandler))
            .inference()
            .described("OpenAI", "Embed text into vectors"),
        GatewayRoute::new("POST", "/api/embed", Box::new(OllamaEmbedHandler))
            .inference()
            .described("Ollama", "Embed text, Ollama-style"),
        GatewayRoute::new("POST", "/api/embeddings", Box::new(OllamaEmbedHandler))
            .inference()
            .described("Ollama", "Embed text (legacy endpoint)"),
        GatewayRoute::new(
            "POST",
            "/v1/completions",
            Box::new(OpenAICompletionsHandler),
        )
        .inference()
        .described("OpenAI", "Complete a text prompt"),
        GatewayRoute::new("POST", "/api/generate", Box::new(OllamaGenerateHandler))
            .inference()
            .described("Ollama", "Generate from a prompt, Ollama-style"),
        GatewayRoute::new(
            "POST",
            "/v1/images/generations",
            Box::new(OpenAIImagesHandler),
        )
        .inference()
        .described("OpenAI", "Generate an image from a prompt"),
        GatewayRoute::new("POST", "/v1/audio/speech", Box::new(OpenAISpeechHandler))
            .inference()
            .described("OpenAI", "Synthesize speech from text"),
    ]
}

/// Releases an inference slot when the request finishes, even on error or unwind.
struct InflightGuard<'a> {
    counter: &'a GatewayCounter,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.counter.exit();
    }
}

/// Routes requests to handlers, enforcing auth, the inference cap, and auditing.
pub struct GatewayRouter {
    port: Arc<dyn GatewayPort>,
    routes: Vec<GatewayRoute>,
    auth: Box<dyn Authenticating>,
    audit: Box<dyn Auditing>,
    max_concurrent_inference: usize,
    inflight: GatewayCounter,
}

impl GatewayRouter {
    /// A router over `port`, using `auth`/`audit` and the given `routes`.
    pub fn new(
        port: Arc<dyn GatewayPort>,
        auth: Box<dyn Authenticating>,
        audit: Box<dyn Auditing>,
        routes: Vec<GatewayRoute>,
        max_concurrent_inference: usize,
    ) -> Self {
        Self {
            port,
            routes,
            auth,
            audit,
            max_concurrent_inference,
            inflight: GatewayCounter::new(),
        }
    }

    /// The body-size limit for `uri`'s route, or `default` if it sets none.
    pub fn body_limit(&self, uri: &str, default: usize) -> usize {
        let path = GatewayRequest::new("GET", uri, Vec::new(), Vec::new()).path;
        self.routes
            .iter()
            .find(|route| route.path == path)
            .and_then(|route| route.max_body_bytes)
            .unwrap_or(default)
    }

    /// Serve one request: authenticate, route, audit, and render an error if the
    /// handler failed before it began responding.
    pub async fn dispatch(&self, request: GatewayRequest, responder: &GatewayResponder) {
        let started = now_unix_millis();
        let surface = GatewaySurface::for_path(&request.path);

        let identity = match self.auth.authenticate(&request).await {
            Ok(identity) => identity,
            Err(error) => {
                let entry = self.error_entry(&request, None, &error, started);
                if error.status() == 401 {
                    self.audit.append_unauthorized(entry);
                } else {
                    self.audit.append(entry);
                }
                self.render(&error, surface, responder);
                return;
            }
        };

        match self.route(&request, &identity, responder).await {
            Ok(outcome) => self
                .audit
                .append(self.ok_entry(&request, &identity, &outcome, started)),
            Err(error) => {
                self.audit
                    .append(self.error_entry(&request, Some(&identity), &error, started));
                if !responder.has_started() {
                    self.render(&error, surface, responder);
                }
            }
        }
    }

    async fn route(
        &self,
        request: &GatewayRequest,
        identity: &GatewayIdentity,
        responder: &GatewayResponder,
    ) -> Result<GatewayOutcome, GatewayError> {
        let mut matched = self
            .routes
            .iter()
            .filter(|route| route.path == request.path);
        if matched.clone().next().is_none() {
            return Err(GatewayError::new(
                GatewayErrorKind::NotFound,
                format!("no route for {}", request.path),
            ));
        }
        let route = matched
            .find(|route| route.method == request.method)
            .ok_or_else(|| {
                GatewayError::new(
                    GatewayErrorKind::MethodNotAllowed,
                    format!("{} not allowed on {}", request.method, request.path),
                )
            })?;

        let _guard = if route.inference {
            if !self.inflight.enter(self.max_concurrent_inference) {
                return Err(GatewayError::new(
                    GatewayErrorKind::Overloaded,
                    "too many requests are already running — retry shortly",
                )
                .with_retry_after(SATURATED_RETRY_AFTER_SECONDS));
            }
            Some(InflightGuard {
                counter: &self.inflight,
            })
        } else {
            None
        };

        route
            .handler
            .handle(request, identity, self.port.as_ref(), responder)
            .await
    }

    fn render(&self, error: &GatewayError, surface: GatewaySurface, responder: &GatewayResponder) {
        let mut extra_headers = Vec::new();
        if let Some(retry) = error.retry_after_seconds {
            extra_headers.push(("Retry-After".to_owned(), retry.to_string()));
        }
        responder.respond(
            error.status(),
            "application/json",
            error.body_bytes(surface),
            extra_headers,
        );
    }

    fn ok_entry(
        &self,
        request: &GatewayRequest,
        identity: &GatewayIdentity,
        outcome: &GatewayOutcome,
        started: i64,
    ) -> GatewayAuditEntry {
        let now = now_unix_millis();
        GatewayAuditEntry {
            ts_millis: now,
            client: Some(identity.client_id.clone()),
            client_name: Some(identity.name.clone()),
            method: request.method.clone(),
            route: request.path.clone(),
            model: outcome.model.clone(),
            capability: outcome.capability.clone(),
            outcome: outcome.outcome.clone(),
            status: outcome.status,
            duration_ms: now - started,
            detail: None,
        }
    }

    fn error_entry(
        &self,
        request: &GatewayRequest,
        identity: Option<&GatewayIdentity>,
        error: &GatewayError,
        started: i64,
    ) -> GatewayAuditEntry {
        let now = now_unix_millis();
        // A server error's message can carry internals; keep it only in the audit
        // detail, never on the wire.
        let detail = (error.kind == GatewayErrorKind::ServerError).then(|| error.message.clone());
        GatewayAuditEntry {
            ts_millis: now,
            client: identity.map(|identity| identity.client_id.clone()),
            client_name: identity.map(|identity| identity.name.clone()),
            method: request.method.clone(),
            route: request.path.clone(),
            model: None,
            capability: None,
            outcome: error.audit_outcome().to_owned(),
            status: error.status(),
            duration_ms: now - started,
            detail,
        }
    }
}
