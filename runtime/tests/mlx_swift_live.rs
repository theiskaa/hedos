//! Live end-to-end smoke test for the in-process MLX-Swift bridge. Ignored by
//! default: it needs the shim built into the binary and a real MLX-safetensors
//! text model, whose snapshot directory is passed via `HEDOS_MLX_TEST_MODEL`.
//!
//! Run with:
//! `HEDOS_MLX_TEST_MODEL=<snapshot-dir> cargo test -p hedos-runtime \
//!    --test mlx_swift_live -- --ignored --nocapture`

#![cfg(target_os = "macos")]

use kernel::capabilities::CapabilityChunk;
use kernel::records::{
    Capability, JsonValue, Modality, ModelRecord, ModelSource, RuntimeId, SourceKind,
};
use runtime::adapters::{MlxSwiftAdapter, RuntimeAdapter, loaded_mlx_swift_backend};
use runtime::governor::{GovernorConfig, MemoryGovernor};

#[tokio::test]
#[ignore = "needs a real MLX text model via HEDOS_MLX_TEST_MODEL"]
async fn a_real_mlx_model_answers_a_chat_prompt() {
    let Ok(model_dir) = std::env::var("HEDOS_MLX_TEST_MODEL") else {
        eprintln!("set HEDOS_MLX_TEST_MODEL to a model snapshot directory");
        return;
    };
    let backend = loaded_mlx_swift_backend();
    assert!(
        backend.is_available(),
        "the MLX-Swift shim must be built into this binary and loadable"
    );
    let adapter = MlxSwiftAdapter::new(MemoryGovernor::new(GovernorConfig::detect()), backend);

    let mut record = ModelRecord::new(
        "qwen-mlx",
        Modality::text(),
        vec![Capability::chat()],
        ModelSource::new(SourceKind::folder(), &model_dir),
    );
    record.runtime.id = Some(RuntimeId::mlx_swift());

    let message = JsonValue::Object(
        [
            ("role".to_owned(), JsonValue::String("user".to_owned())),
            (
                "content".to_owned(),
                JsonValue::String("Reply with exactly the word: pong".to_owned()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let payload = JsonValue::Object(
        [
            ("messages".to_owned(), JsonValue::Array(vec![message])),
            ("max_tokens".to_owned(), JsonValue::Int(32)),
        ]
        .into_iter()
        .collect(),
    );

    let mut stream = adapter.invoke(&record, Capability::chat(), payload);
    let mut text = String::new();
    let mut done = false;
    while let Some(item) = stream.recv().await {
        match item.expect("the generation must not error") {
            CapabilityChunk::Text(chunk) => text.push_str(&chunk),
            CapabilityChunk::Done(_) => done = true,
            _ => {}
        }
    }
    eprintln!("MLX-Swift reply: {text:?}");
    assert!(done, "the stream must end with a Done chunk");
    assert!(
        !text.trim().is_empty(),
        "the model must produce visible text"
    );
}
