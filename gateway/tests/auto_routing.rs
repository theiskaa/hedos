//! The `auto` model end to end against a mock port: every chat and completion
//! dialect routes a simple turn to the small model and a demanding one to the
//! large, says so in its headers and the audit log, and keeps a conversation
//! that went large there.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{MockPort, invoked_names, routed_port};
use gateway::admission::GatewayAdmissionState;
use gateway::audit::{Auditing, GatewayAuditEntry};
use gateway::auth::OpenAuth;
use gateway::auto_route::AutoRoute;
use gateway::error::GatewayErrorKind;
use gateway::handlers::chat::{OllamaChatHandler, OpenAIChatHandler};
use gateway::handlers::generate::{OllamaGenerateHandler, OpenAICompletionsHandler};
use gateway::handlers::messages::AnthropicMessagesHandler;
use gateway::handlers::{GatewayHandling, ROUTE_REASON_HEADER, ROUTED_MODEL_HEADER};
use gateway::identity::GatewayIdentity;
use gateway::port::GatewayPort;
use gateway::request::GatewayRequest;
use gateway::responder::{GatewayResponder, ResponsePart};
use gateway::router::{GatewayRouter, standard_routes};
use gateway::scopes::GatewayScopes;
use kernel::records::Capability;
use kernel::routing::RouteRules;

fn identity() -> GatewayIdentity {
    GatewayIdentity::new("client", "Client", GatewayScopes::all())
}

fn request(uri: &str, body: &str) -> GatewayRequest {
    GatewayRequest::new("POST", uri, Vec::new(), body.as_bytes().to_vec())
}

struct Reply {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn collect(mut rx: tokio::sync::mpsc::UnboundedReceiver<ResponsePart>) -> Reply {
    let mut reply = Reply {
        status: None,
        headers: Vec::new(),
        body: String::new(),
    };
    let mut body = Vec::new();
    while let Ok(part) = rx.try_recv() {
        match part {
            ResponsePart::Head { status, headers } => {
                reply.status = Some(status);
                reply.headers = headers;
            }
            ResponsePart::Chunk(bytes) => body.extend(bytes),
        }
    }
    reply.body = String::from_utf8_lossy(&body).into_owned();
    reply
}

async fn send(
    handler: &dyn GatewayHandling,
    port: &MockPort,
    uri: &str,
    body: &str,
) -> Result<Reply, gateway::GatewayError> {
    let (responder, rx) = GatewayResponder::new();
    handler
        .handle(&request(uri, body), &identity(), port, &responder)
        .await?;
    Ok(collect(rx))
}

fn openai_chat(text: &str, stream: bool) -> String {
    serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": text}],
        "stream": stream,
    })
    .to_string()
}

#[tokio::test]
async fn openai_chat_routes_by_the_turn_streamed_and_not() {
    let port = routed_port();
    let handler = OpenAIChatHandler::default();
    for stream in [false, true] {
        let simple = send(
            &handler,
            &port,
            "/v1/chat/completions",
            &openai_chat("what is 2+2?", stream),
        )
        .await
        .unwrap();
        assert_eq!(simple.status, Some(200));
        assert_eq!(simple.header(ROUTED_MODEL_HEADER), Some("tiny"));
        assert_eq!(simple.header(ROUTE_REASON_HEADER), Some("short turn"));
        assert!(
            simple.body.contains("\"model\":\"auto\""),
            "{}",
            simple.body
        );

        let hard = send(
            &handler,
            &port,
            "/v1/chat/completions",
            &openai_chat("refactor the session store", stream),
        )
        .await
        .unwrap();
        assert_eq!(hard.header(ROUTED_MODEL_HEADER), Some("big"));
        assert_eq!(
            hard.header(ROUTE_REASON_HEADER),
            Some("hard word \"refactor\"")
        );
    }
    assert_eq!(invoked_names(&port), ["tiny", "big", "tiny", "big"]);
}

#[tokio::test]
async fn ollama_chat_routes_and_names_the_model_in_its_headers() {
    let port = routed_port();
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": "```\nfn main() {}\n```\nwhy?"}],
    })
    .to_string();
    let reply = send(&OllamaChatHandler::default(), &port, "/api/chat", &body)
        .await
        .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(reply.header(ROUTE_REASON_HEADER), Some("code in turn"));
    assert!(reply.body.contains("\"model\":\"auto\""));
}

#[tokio::test]
async fn anthropic_messages_route_past_the_reminders_claude_code_injects() {
    let mut port = routed_port();
    port.honored.insert("max_tokens".to_owned());
    let reminder = format!(
        "<system-reminder>{}</system-reminder>",
        "project context ".repeat(500)
    );
    let body = serde_json::json!({
        "model": "auto",
        "max_tokens": 64,
        "system": "You are a coding agent.",
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": reminder},
            {"type": "text", "text": "what does this repo do?"},
        ]}],
        "stream": true,
    })
    .to_string();
    let reply = send(
        &AnthropicMessagesHandler::default(),
        &port,
        "/v1/messages",
        &body,
    )
    .await
    .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("tiny"));
    assert_eq!(reply.header(ROUTE_REASON_HEADER), Some("short turn"));
}

#[tokio::test]
async fn both_completion_endpoints_route_their_prompt() {
    let port = routed_port();
    let body = serde_json::json!({"model": "auto", "prompt": "Once upon a time"}).to_string();
    let reply = send(&OpenAICompletionsHandler, &port, "/v1/completions", &body)
        .await
        .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("tiny"));

    let body = serde_json::json!({
        "model": "auto",
        "prompt": "optimize this query",
        "stream": false,
    })
    .to_string();
    let reply = send(&OllamaGenerateHandler, &port, "/api/generate", &body)
        .await
        .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(invoked_names(&port), ["tiny", "big"]);
}

#[tokio::test]
async fn a_conversation_that_went_large_keeps_its_model() {
    let port = routed_port();
    let handler = OpenAIChatHandler::default();
    let first = serde_json::json!({
        "model": "auto",
        "messages": [
            {"role": "system", "content": "be terse"},
            {"role": "user", "content": "debug the flaky login test"},
        ],
    })
    .to_string();
    send(&handler, &port, "/v1/chat/completions", &first)
        .await
        .unwrap();

    let later = serde_json::json!({
        "model": "auto",
        "messages": [
            {"role": "system", "content": "be terse"},
            {"role": "user", "content": "debug the flaky login test"},
            {"role": "assistant", "content": "done"},
            {"role": "user", "content": "thanks"},
        ],
    })
    .to_string();
    let reply = send(&handler, &port, "/v1/chat/completions", &later)
        .await
        .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(
        reply.header(ROUTE_REASON_HEADER),
        Some("conversation already large")
    );
}

#[tokio::test]
async fn tools_the_small_model_cannot_call_go_large() {
    let mut port = routed_port();
    port.shelf[0].capabilities = vec![Capability::chat()];
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": "list the files"}],
        "tools": [{"type": "function", "function": {
            "name": "ls", "description": "list", "parameters": {"type": "object"}
        }}],
    })
    .to_string();
    let reply = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &body,
    )
    .await
    .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(reply.header(ROUTE_REASON_HEADER), Some("needs tools"));
}

#[tokio::test]
async fn a_prompt_past_the_small_window_goes_large() {
    let mut port = routed_port();
    let tiny = port.shelf[0].id.clone();
    port.windows.insert(tiny, 2048);
    // The history is long, the latest turn short: only the window can send it
    // large.
    let history = serde_json::json!({
        "model": "auto",
        "messages": [
            {"role": "user", "content": "y ".repeat(5000)},
            {"role": "assistant", "content": "noted"},
            {"role": "user", "content": "ok?"},
        ],
    })
    .to_string();
    let reply = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &history,
    )
    .await
    .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(
        reply.header(ROUTE_REASON_HEADER),
        Some("needs a wider window")
    );
}

#[tokio::test]
async fn neither_model_ready_is_not_found_naming_both() {
    let mut port = routed_port();
    port.shelf.clear();
    let error = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &openai_chat("hi", false),
    )
    .await
    .err()
    .expect("nothing to route to");
    assert_eq!(error.kind, GatewayErrorKind::NotFound);
    assert!(error.message.contains("tiny (small) is not ready"));
    assert!(error.message.contains("big (large) is not ready"));
}

#[tokio::test]
async fn a_named_model_is_served_as_before() {
    let port = routed_port();
    let body = serde_json::json!({
        "model": "big",
        "messages": [{"role": "user", "content": "hi"}],
    })
    .to_string();
    let reply = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &body,
    )
    .await
    .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), None);
    assert_eq!(reply.header(ROUTE_REASON_HEADER), None);
    assert_eq!(invoked_names(&port), ["big"]);
}

#[tokio::test]
async fn asking_for_auto_with_routing_off_says_how_to_turn_it_on() {
    let mut port = routed_port();
    port.auto_route = None;
    let error = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &openai_chat("hi", false),
    )
    .await
    .err()
    .expect("auto is not a model");
    assert_eq!(error.kind, GatewayErrorKind::NotFound);
    assert_eq!(
        error.message,
        format!(
            "no ready model matches auto; {}",
            runtime::settings::ROUTING_OFF
        )
    );
}

#[tokio::test]
async fn a_header_never_carries_what_a_header_cannot_hold() {
    let mut port = routed_port();
    port.auto_route = Some(Arc::new(AutoRoute::new(
        "auto",
        "tiny",
        "big",
        RouteRules {
            max_simple_chars: 1200,
            hard_words: vec!["résumé".to_owned()],
        },
        Duration::from_secs(60),
    )));
    let reply = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &openai_chat("polish my résumé", false),
    )
    .await
    .unwrap();
    assert_eq!(
        reply.header(ROUTE_REASON_HEADER),
        Some("hard word \"r?sum?\"")
    );
}

#[derive(Default, Clone)]
struct Captured(Arc<Mutex<Vec<GatewayAuditEntry>>>);

impl Auditing for Captured {
    fn append(&self, entry: GatewayAuditEntry) {
        self.0.lock().unwrap().push(entry);
    }
}

#[tokio::test]
async fn the_audit_log_records_the_route() {
    let port = routed_port();
    let big = port.shelf[1].id.clone();
    let captured = Captured::default();
    let router = GatewayRouter::new(
        Arc::new(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(captured.clone()),
        standard_routes(),
        4,
    );
    let (responder, _rx) = GatewayResponder::new();
    router
        .dispatch(
            &request(
                "/v1/chat/completions",
                &openai_chat("investigate the leak", false),
            ),
            &responder,
        )
        .await;
    let (responder, _rx) = GatewayResponder::new();
    let named = serde_json::json!({
        "model": "tiny",
        "messages": [{"role": "user", "content": "hi"}],
    })
    .to_string();
    router
        .dispatch(&request("/v1/chat/completions", &named), &responder)
        .await;

    let entries = captured.0.lock().unwrap().clone();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].model.as_deref(), Some(big.as_str()));
    assert_eq!(entries[0].routed_from.as_deref(), Some("auto"));
    assert_eq!(
        entries[0].route_reason.as_deref(),
        Some("hard word \"investigate\"")
    );
    assert_eq!(entries[1].routed_from, None);
    assert_eq!(entries[1].route_reason, None);
}

#[tokio::test]
async fn an_image_earlier_in_the_conversation_does_not_pin_it() {
    // Only `big` sees. The picture was in the first turn; the runtime only
    // checks the last message for one, and so does routing.
    let mut port = routed_port();
    port.shelf[1].capabilities.push(Capability::see());
    let body = serde_json::json!({
        "model": "auto",
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "what is this?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}},
            ]},
            {"role": "assistant", "content": "a cat"},
            {"role": "user", "content": "thanks"},
        ],
    })
    .to_string();
    let reply = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &body,
    )
    .await
    .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("tiny"));
}

#[tokio::test]
async fn a_parameter_the_small_runtime_does_not_take_goes_large() {
    let mut port = routed_port();
    port.honored.insert("temperature".to_owned());
    port.honored_by.insert(
        port.shelf[1].id.clone(),
        ["temperature", "presence_penalty"]
            .map(str::to_owned)
            .into(),
    );
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": "hi"}],
        "presence_penalty": 0.5,
    })
    .to_string();
    let reply = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &body,
    )
    .await
    .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(
        reply.header(ROUTE_REASON_HEADER),
        Some("needs presence_penalty")
    );
}

#[tokio::test]
async fn asking_for_auto_while_routing_answers_another_name_gives_no_hint() {
    let mut port = routed_port();
    port.auto_route = Some(Arc::new(AutoRoute::new(
        "router",
        "tiny",
        "big",
        RouteRules::default(),
        Duration::from_secs(60),
    )));
    let error = send(
        &OpenAIChatHandler::default(),
        &port,
        "/v1/chat/completions",
        &openai_chat("hi", false),
    )
    .await
    .err()
    .expect("auto is not a model here");
    assert_eq!(error.message, "no ready model matches auto");
}

#[tokio::test]
async fn a_refused_routed_request_is_still_audited_as_routed() {
    let mut port = routed_port();
    port.shelf.clear();
    let captured = Captured::default();
    let router = GatewayRouter::new(
        Arc::new(port) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(captured.clone()),
        standard_routes(),
        4,
    );
    let (responder, _rx) = GatewayResponder::new();
    router
        .dispatch(
            &request("/v1/chat/completions", &openai_chat("hi", false)),
            &responder,
        )
        .await;
    let entries = captured.0.lock().unwrap().clone();
    assert_eq!(entries[0].outcome, "not_found");
    assert_eq!(entries[0].routed_from.as_deref(), Some("auto"));
}

#[tokio::test]
async fn a_request_that_fails_after_routing_does_not_pin_its_conversation() {
    // Two ports over one routing: the first cannot admit work, the second can.
    let ready = routed_port();
    let busy = MockPort {
        shelf: ready.shelf.clone(),
        admission: GatewayAdmissionState::Saturated {
            retry_after_seconds: 1,
        },
        auto_route: ready.auto_route.clone(),
        ..MockPort::default()
    };
    let handler = OpenAIChatHandler::default();
    let hard = openai_chat("refactor the parser", false);
    let refused = send(&handler, &busy, "/v1/chat/completions", &hard).await;
    assert_eq!(
        refused.err().map(|error| error.kind),
        Some(GatewayErrorKind::Overloaded)
    );

    let later = serde_json::json!({
        "model": "auto",
        "messages": [
            {"role": "user", "content": "refactor the parser"},
            {"role": "assistant", "content": "busy"},
            {"role": "user", "content": "never mind, what time is it?"},
        ],
    })
    .to_string();
    let reply = send(&handler, &ready, "/v1/chat/completions", &later)
        .await
        .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("tiny"));
    assert_eq!(reply.header(ROUTE_REASON_HEADER), Some("short turn"));

    // Once a hard turn is served, the conversation stays large.
    send(&handler, &ready, "/v1/chat/completions", &hard)
        .await
        .unwrap();
    let reply = send(&handler, &ready, "/v1/chat/completions", &later)
        .await
        .unwrap();
    assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("big"));
    assert_eq!(
        reply.header(ROUTE_REASON_HEADER),
        Some("conversation already large")
    );
}

#[tokio::test]
async fn the_routed_name_is_matched_as_a_model_name_is() {
    let port = routed_port();
    for name in ["Auto", "auto:latest"] {
        let body = serde_json::json!({
            "model": name,
            "messages": [{"role": "user", "content": "hi"}],
        })
        .to_string();
        let reply = send(
            &OpenAIChatHandler::default(),
            &port,
            "/v1/chat/completions",
            &body,
        )
        .await
        .unwrap();
        assert_eq!(reply.header(ROUTED_MODEL_HEADER), Some("tiny"), "{name}");
    }
}

#[tokio::test]
async fn a_failed_request_to_a_route_that_does_not_route_is_not_counted_as_routed() {
    let captured = Captured::default();
    let router = GatewayRouter::new(
        Arc::new(routed_port()) as Arc<dyn GatewayPort>,
        Box::new(OpenAuth),
        Box::new(captured.clone()),
        standard_routes(),
        4,
    );
    let (responder, _rx) = GatewayResponder::new();
    router
        .dispatch(
            &request("/v1/embeddings", r#"{"model":"auto","input":"hi"}"#),
            &responder,
        )
        .await;
    let entries = captured.0.lock().unwrap().clone();
    assert_ne!(entries[0].outcome, "ok");
    assert_eq!(entries[0].routed_from, None);
}
