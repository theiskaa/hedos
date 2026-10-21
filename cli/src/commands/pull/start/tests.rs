use super::*;

use kernel::install::pulls::{PullControl, PullState};

use crate::support::pulls::testing::{TempDir, job as make_job};

fn out() -> Out {
    Out::new(false)
}

#[tokio::test]
async fn rejoining_a_stopped_pull_that_carries_a_cancel_clears_the_way_instead() {
    let directory = TempDir::new("start-rejoin-cancel");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    job.update_status(1_000, |status| status.state = PullState::Paused)
        .expect("write the record");
    job.request(PullControl::Cancel).expect("write the ask");

    assert_eq!(rejoin(&out(), &job, true).await.expect("rejoin"), None);
    assert_eq!(job.stored_status().state, PullState::Cancelled);
}

#[tokio::test]
async fn rejoining_a_pull_still_going_leaves_it_to_its_worker() {
    let directory = TempDir::new("start-rejoin-going");
    let store = directory.store();
    let job = make_job(&store, "Qwen/Qwen3-8B", 1_000);
    let _worker = job.claim().expect("claim").expect("the lock is free");
    job.update_status(1_000, |status| {
        status.state = PullState::Running;
        status.pid = Some(std::process::id());
    })
    .expect("write the record");

    assert_eq!(
        rejoin(&out(), &job, true).await.expect("rejoin"),
        Some(Seen::Otherwise),
        "left to its worker"
    );
    assert_eq!(job.stored_status().state, PullState::Running);
}

#[test]
fn only_a_model_llama_cpp_serves_is_offered_llama_cpp_after_a_pull() {
    use kernel::records::{Modality, ModelSource, RuntimeId, SourceKind};
    let record = |repo: &str, runtime: RuntimeId| {
        let name = repo.rsplit('/').next().unwrap();
        let mut source = ModelSource::new(SourceKind::huggingface_cache(), &format!("/hub/{repo}"));
        source.repo = Some(repo.to_owned());
        let mut record = ModelRecord::new(name, Modality::text(), Vec::new(), source);
        record.runtime.id = Some(runtime);
        record
    };
    let shelf = [
        // Another owner's repo of the same name, served by something else.
        record("someone/gemma-4-E4B-it-GGUF", RuntimeId::ollama()),
        record("ggml-org/gemma-4-E4B-it-GGUF", RuntimeId::llama_cpp()),
        record("stabilityai/sdxl-turbo", RuntimeId::diffusers()),
        record("ggerganov/whisper.cpp", RuntimeId::whisper_cpp()),
    ];
    assert_eq!(
        served_by_llama_cpp(&shelf, "ggml-org/gemma-4-E4B-it-GGUF"),
        Some("gemma-4-E4B-it-GGUF")
    );
    assert_eq!(
        served_by_llama_cpp(&shelf, "someone/gemma-4-E4B-it-GGUF"),
        None
    );
    assert_eq!(served_by_llama_cpp(&shelf, "stabilityai/sdxl-turbo"), None);
    assert_eq!(served_by_llama_cpp(&shelf, "ggerganov/whisper.cpp"), None);
    assert_eq!(served_by_llama_cpp(&shelf, "org/not-pulled"), None);
}
