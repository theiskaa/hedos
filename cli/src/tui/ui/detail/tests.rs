use super::*;

use crate::tui::facts::ModelActivity;
use crate::tui::testing::{
    facts_with_memory, leading_label, record_with, resident_with_bytes, text, texts,
};
use gateway::stats::LatencyPercentiles;

#[test]
fn every_label_is_listed() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.alias = Some("alias".to_owned());
    record.primary_weight_path = Some("/models/m.gguf".to_owned());
    let mut facts = Facts {
        collected_at_millis: 1_000_000,
        ..facts_with_memory(64)
    };
    facts.activity.models.insert(
        record.id.clone(),
        ModelActivity {
            requests: 3,
            latency: Some(LatencyPercentiles {
                p50: 1,
                p90: 2,
                p99: 3,
            }),
            hourly: [0; HOURS],
            last_seen_millis: 500_000,
        },
    );
    let mut seen = 0;
    for line in full_lines(&record, &facts, true, 80) {
        let label = leading_label(&line, label_column());
        if label.is_empty() || label.chars().all(|c| c.is_uppercase()) {
            continue;
        }
        assert!(LABELS.contains(&label.as_str()), "{label} is not listed");
        seen += 1;
    }
    // Every label drawn is listed, and with the alias, path, and gateway
    // traffic set, the expanded pane draws every label listed.
    assert_eq!(seen, LABELS.len());
}

#[test]
fn a_gone_record_says_so_on_path_and_fit() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_mb = Some(4 * 1024);
    record.primary_weight_path = Some("/models/m.gguf".to_owned());
    record.state = ModelState::Missing;
    let facts = facts_with_memory(64);
    let lines = full_lines(&record, &facts, false, 80);
    let path = lines
        .iter()
        .find(|line| text(line).starts_with(" path"))
        .expect("a path row");
    assert!(
        text(path).ends_with("/models/m.gguf · gone"),
        "{:?}",
        text(path)
    );
    assert_eq!(path.spans.last().map(|span| span.style), Some(DIM));
    let fit = lines
        .iter()
        .map(text)
        .find(|line| line.starts_with(" fit"))
        .unwrap_or_default();
    assert!(fit.contains("weights are gone · fits · needs"), "{fit:?}");
}

#[test]
fn long_values_are_clipped_to_the_pane() {
    let caps = [
        "chat",
        "complete",
        "embed",
        "see",
        "image",
        "speak",
        "transcribe",
        "tools",
    ];
    let mut record = record_with("m", caps.into_iter().map(Capability::from).collect());
    record.footprint_mb = Some(4 * 1024);
    record.primary_weight_path = Some(format!("/models/{}.gguf", "x".repeat(80)));
    let mut gateway = resident_with_bytes(&record.id, Holder::Gateway, 4 << 30);
    gateway.expires_at_millis = Some(i64::MAX / 2);
    let facts = Facts {
        gateway_port: Some(11434),
        residents: vec![
            gateway,
            resident_with_bytes("other", Holder::Local, 30 << 30),
        ],
        ..facts_with_memory(64)
    };
    let lines = full_lines(&record, &facts, true, 40);
    for line in &lines {
        assert!(line.width() <= 40, "{:?} runs past the pane", text(line));
    }
    let find = |label: &str| {
        lines
            .iter()
            .map(text)
            .find(|line| line.starts_with(&format!(" {label}")))
            .unwrap_or_default()
    };
    assert!(find("caps").ends_with('…'));
    assert!(find("fit").ends_with('…'));
    assert!(find("residency").contains("warm") && find("residency").ends_with('…'));
    assert!(find("path").contains('…') && find("path").ends_with(".gguf"));
    assert!(texts(&lines).contains(&" RECORD".to_owned()));
}

#[test]
fn the_compact_detail_skips_what_the_row_shows() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_mb = Some(4 * 1024);
    record.primary_weight_path = Some("/models/m.gguf".to_owned());
    let mut facts = Facts {
        collected_at_millis: 1_000_000,
        ..facts_with_memory(64)
    };
    let labels_of = |lines: &[Line]| -> Vec<String> {
        lines
            .iter()
            .map(|line| leading_label(line, label_column()))
            .collect()
    };
    let quiet = compact_lines(&record, &facts, 80);
    assert_eq!(labels_of(&quiet), ["fit", "residency", "last 24h", "path"]);
    assert!(text(&quiet[2]).contains("no requests through the gateway"));

    facts.activity.models.insert(
        record.id.clone(),
        ModelActivity {
            requests: 0,
            latency: None,
            hourly: [0; HOURS],
            last_seen_millis: 500_000,
        },
    );
    let idle = compact_lines(&record, &facts, 80);
    assert_eq!(labels_of(&idle), ["fit", "residency", "last used", "path"]);
    assert!(text(&idle[2]).ends_with("ago"));

    facts.activity.models.get_mut(&record.id).unwrap().requests = 12;
    let busy = compact_lines(&record, &facts, 80);
    assert_eq!(labels_of(&busy)[2], "last 24h");
    assert!(text(&busy[2]).contains("12 requests served"));

    record.primary_weight_path = None;
    let pathless = compact_lines(&record, &facts, 80);
    assert_eq!(
        labels_of(&pathless),
        ["fit", "residency", "last 24h", "size"]
    );
    assert!(text(&pathless[3]).contains("4 GB"));

    let full = full_lines(&record, &facts, false, 80);
    assert!(labels_of(&full).contains(&"runtime".to_owned()));
    assert!(full.len() > STACKED_DETAIL_ROWS as usize);
    assert!(!texts(&full).contains(&" RECORD".to_owned()));
}
