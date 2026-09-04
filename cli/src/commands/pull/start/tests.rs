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

    assert!(!rejoin(&out(), &job, true).await.expect("rejoin"));
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

    assert!(rejoin(&out(), &job, true).await.expect("rejoin"));
    assert_eq!(job.stored_status().state, PullState::Running);
}
