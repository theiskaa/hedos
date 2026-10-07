//! `hedos disk-count`, the process the shelf counts its bytes on disk in:
//! the shelf's records go in on stdin, and the bytes per store come out as
//! JSON, each file counted once.

use std::io::Write;
use std::process::{Command, Stdio};

use kernel::records::{Capability, Modality, ModelRecord, ModelSource, SourceKind};

fn count(records: &[ModelRecord]) -> serde_json::Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hedos"))
        .arg("disk-count")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(records).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn the_records_on_stdin_are_counted_per_store_each_file_once() {
    let dir = std::env::temp_dir().join(format!("hedos-disk-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.gguf");
    std::fs::write(&file, [1u8; 700]).unwrap();
    let record = |kind: SourceKind| {
        ModelRecord::new(
            "a",
            Modality::text(),
            vec![Capability::chat()],
            ModelSource::new(kind, &file.to_string_lossy()),
        )
    };
    let counted = count(&[record(SourceKind::file()), record(SourceKind::lm_studio())]);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(
        counted,
        serde_json::json!([["lm-studio", 700], ["file", 0]])
    );
    assert_eq!(count(&[]), serde_json::json!([]));
}
