//! Tests for the artifact store, gallery arrangement, and provenance rendering.

mod support;

use kernel::artifacts::{
    Artifact, ArtifactDraft, ArtifactStore, ArtifactStoreError, Gallery, GalleryModel, GallerySort,
    Provenance,
};
use kernel::records::{Capability, JsonValue, ParamSpec, ParamType};
use support::TempDir;

fn params(seed: i64) -> JsonValue {
    JsonValue::Object(
        [
            (
                "prompt".to_owned(),
                JsonValue::String("a lighthouse".to_owned()),
            ),
            ("steps".to_owned(), JsonValue::Int(4)),
            ("seed".to_owned(), JsonValue::Int(seed)),
        ]
        .into_iter()
        .collect(),
    )
}

fn draft(model: &str, job_id: &str, data: &[u8], preview: Option<&[u8]>) -> ArtifactDraft {
    ArtifactDraft {
        data: data.to_vec(),
        file_extension: "png".to_owned(),
        preview: preview.map(<[u8]>::to_vec),
        model: model.to_owned(),
        model_id: "flux".to_owned(),
        runtime: "fake:image".to_owned(),
        capability: Capability::image(),
        params: params(7),
        job_id: job_id.to_owned(),
        duration_ms: 5,
        session_id: None,
    }
}

#[test]
fn store_writes_dedups_and_round_trips_on_reload() {
    let dir = TempDir::new();
    let mut store = ArtifactStore::new(dir.path());

    let a = store
        .store(draft(
            "FLUX.1-schnell",
            "job-aaaa",
            b"png-a",
            Some(b"thumb-a"),
        ))
        .expect("store a");
    let b = store
        .store(draft("FLUX.1-schnell", "job-bbbb", b"png-b", None))
        .expect("store b");

    assert!(a.path.ends_with(".png"));
    assert!(a.path.contains('/'), "path is year-prefixed");
    assert_eq!(
        a.preview_path.as_deref().map(|p| p.starts_with("blobs/")),
        Some(true)
    );
    assert_eq!(b.preview_path, None);

    let blob = std::fs::read(dir.path().join(&a.path)).expect("output written");
    assert_eq!(blob, b"png-a");
    assert_eq!(
        store.preview_data(&a.id).expect("preview"),
        Some(b"thumb-a".to_vec())
    );
    assert_eq!(store.preview_data(&b.id).expect("preview"), None);
    assert_eq!(
        store.url(&a.id).expect("url"),
        Some(dir.path().join(&a.path))
    );
    assert_eq!(store.url("missing").expect("url"), None);

    let mut reloaded = ArtifactStore::new(dir.path());
    let ids: std::collections::HashSet<String> = reloaded
        .list()
        .expect("list")
        .into_iter()
        .map(|art| art.id)
        .collect();
    assert_eq!(ids, [a.id.clone(), b.id.clone()].into_iter().collect());
}

#[test]
fn identical_bytes_get_distinct_ids_but_share_the_output_file() {
    let dir = TempDir::new();
    let mut store = ArtifactStore::new(dir.path());
    let one = store
        .store(draft("m", "job-same", b"frame", None))
        .expect("one");
    let two = store
        .store(draft("m", "job-same", b"frame", None))
        .expect("two");

    assert_ne!(one.id, two.id, "distinct ids");
    assert_eq!(one.path, two.path, "shared, deduplicated output file");
    assert_eq!(one.content_hash, two.content_hash);
    assert_eq!(store.list().expect("list").len(), 2);
}

#[test]
fn list_is_newest_first_across_reload() {
    let dir = TempDir::new();
    let mut store = ArtifactStore::new(dir.path());
    let mut ids = Vec::new();
    for index in 0..3 {
        let artifact = store
            .store(draft(
                "m",
                &format!("job-{index}"),
                format!("frame-{index}").as_bytes(),
                None,
            ))
            .expect("store");
        ids.push(artifact.id);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let listed: Vec<String> = store
        .list()
        .expect("list")
        .into_iter()
        .map(|a| a.id)
        .collect();
    let mut expected = ids.clone();
    expected.reverse();
    assert_eq!(listed, expected, "newest submitted first");

    let mut reloaded = ArtifactStore::new(dir.path());
    let reloaded_ids: Vec<String> = reloaded
        .list()
        .expect("list")
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(reloaded_ids, expected);
}

#[test]
fn delete_removes_sidecar_always_and_output_only_when_unshared() {
    let dir = TempDir::new();
    let mut store = ArtifactStore::new(dir.path());
    let original = store
        .store(draft("m", "job-one", b"frame", None))
        .expect("one");
    let duplicate = store
        .store(draft("m", "job-two", b"frame", None))
        .expect("two");
    let output = dir.path().join(&original.path);
    assert_eq!(original.path, duplicate.path, "same bytes share the file");

    store.delete(&duplicate.id).expect("delete duplicate");
    assert_eq!(store.list().expect("list").len(), 1);
    assert!(
        output.exists(),
        "the shared output stays while an owner remains"
    );

    store.delete(&original.id).expect("delete original");
    assert!(store.list().expect("list").is_empty());
    assert!(!output.exists(), "the last owner gone removes the output");

    assert!(matches!(
        store.delete(&original.id),
        Err(ArtifactStoreError::NotFound(_))
    ));
}

fn stub(id: &str, model: &str, model_id: &str, created_at: i64) -> Artifact {
    Artifact {
        id: id.to_owned(),
        path: format!("2026/{id}.png"),
        content_hash: id.to_owned(),
        preview_path: None,
        model: model.to_owned(),
        model_id: model_id.to_owned(),
        runtime: "fake:image".to_owned(),
        capability: Capability::image(),
        params: params(7),
        created_at,
        duration_ms: 100,
        job_id: "job".to_owned(),
        session_id: None,
    }
}

#[test]
fn gallery_models_are_distinct_and_ordered_by_recency() {
    let artifacts = vec![
        stub("a", "FLUX.1-schnell", "flux", 1000),
        stub("b", "sdxl-turbo", "sdxl", 1020),
        stub("c", "FLUX.1-schnell", "flux", 1040),
    ];
    assert_eq!(
        Gallery::models(&artifacts),
        vec![
            GalleryModel {
                id: "flux".to_owned(),
                name: "FLUX.1-schnell".to_owned()
            },
            GalleryModel {
                id: "sdxl".to_owned(),
                name: "sdxl-turbo".to_owned()
            },
        ]
    );
    assert!(Gallery::models(&[]).is_empty());
}

#[test]
fn gallery_arrange_filters_and_sorts_both_ways_with_stable_tiebreak() {
    let artifacts = vec![
        stub("old", "m", "flux", 1000),
        stub("tie-b", "m", "flux", 1020),
        stub("tie-a", "m", "flux", 1020),
        stub("new", "m", "sdxl", 1040),
    ];
    let ids = |arts: Vec<Artifact>| arts.into_iter().map(|a| a.id).collect::<Vec<_>>();

    assert_eq!(
        ids(Gallery::arrange(
            &artifacts,
            Some("flux"),
            GallerySort::NewestFirst
        )),
        vec!["tie-b", "tie-a", "old"]
    );
    assert!(Gallery::arrange(&artifacts, Some("missing"), GallerySort::NewestFirst).is_empty());
    assert_eq!(
        Gallery::arrange(&artifacts, None, GallerySort::NewestFirst).len(),
        4
    );

    assert_eq!(
        ids(Gallery::arrange(&artifacts, None, GallerySort::NewestFirst)),
        vec!["new", "tie-b", "tie-a", "old"]
    );
    assert_eq!(
        ids(Gallery::arrange(&artifacts, None, GallerySort::OldestFirst)),
        vec!["old", "tie-a", "tie-b", "new"]
    );
}

fn spec(key: &str) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        param_type: ParamType::Int,
        default_value: None,
        range: None,
        values: None,
    }
}

#[test]
fn provenance_prompt_and_param_ordering() {
    let value = JsonValue::Object(
        [
            ("prompt".to_owned(), JsonValue::String("hi".to_owned())),
            ("seed".to_owned(), JsonValue::Int(42)),
            ("steps".to_owned(), JsonValue::Int(4)),
            ("guidance".to_owned(), JsonValue::Double(0.0)),
        ]
        .into_iter()
        .collect(),
    );
    assert_eq!(Provenance::prompt(&value).as_deref(), Some("hi"));

    let artifact = Artifact {
        params: value,
        ..stub("a", "FLUX.1-schnell", "flux", 1000)
    };
    // Schema keys first in schema order (steps, guidance), then extras alpha
    // (seed); prompt is excluded from the pairs.
    let line = Provenance::line(&artifact, &[spec("steps"), spec("guidance")]);
    assert_eq!(
        line,
        "FLUX.1-schnell · steps 4 · guidance 0 · seed 42 · 100 ms"
    );

    let details = Provenance::details(&artifact, &[spec("steps")]);
    assert!(details.starts_with("model: FLUX.1-schnell\nruntime: fake:image\ncapability: image"));
    assert!(details.contains("prompt: hi"));
    assert!(details.contains("job: job"));
}

#[test]
fn provenance_duration_rendering() {
    assert_eq!(Provenance::duration(850), "850 ms");
    assert_eq!(Provenance::duration(1200), "1.2s");
    assert_eq!(Provenance::duration(180_000), "3m");
    assert_eq!(Provenance::duration(185_000), "3m 5s");
}
