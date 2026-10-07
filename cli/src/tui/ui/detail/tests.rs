use super::*;

use crate::tui::facts::ModelActivity;
use crate::tui::testing::{
    facts_with_memory, leading_label, record_with, resident_with_bytes, text, texts,
};
use gateway::stats::LatencyPercentiles;

/// Whether `line` starts with a label from the label column, as a row does
/// and a heading, the name, or the chips do not.
fn labelled(line: &Line) -> bool {
    line.spans
        .first()
        .is_some_and(|span| span.style == DIM && span.content.width() == label_column() + 1)
}

fn settled() -> Motion {
    Motion::settled()
}

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
    let motion = settled();
    for line in full_lines(&record, &facts, true, 80, &Look::settled(&motion)) {
        if !labelled(&line) {
            continue;
        }
        let label = leading_label(&line, label_column());
        if label.is_empty() {
            continue;
        }
        assert!(LABELS.contains(&label.as_str()), "{label} is not listed");
        seen += 1;
    }
    // Every label drawn is listed, and with the alias, path, and gateway
    // traffic set, the expanded card draws every label listed but the size,
    // which only the stacked card's pathless row uses.
    assert_eq!(seen, LABELS.len() - 1);
}

#[test]
fn a_gone_record_says_so_on_path_and_fit() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_bytes = Some(4 * (1 << 30));
    record.primary_weight_path = Some("/models/m.gguf".to_owned());
    record.state = ModelState::Missing;
    let facts = facts_with_memory(64);
    let motion = settled();
    let lines = full_lines(&record, &facts, false, 80, &Look::settled(&motion));
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
    record.footprint_bytes = Some(4 * (1 << 30));
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
    let motion = settled();
    let lines = full_lines(&record, &facts, true, 40, &Look::settled(&motion));
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
    let chips = lines
        .iter()
        .map(text)
        .find(|line| line.contains(" chat "))
        .unwrap_or_default();
    assert!(
        chips.contains(" complete ") && !chips.contains("transcribe"),
        "{chips:?}"
    );
    assert!(find("fit").ends_with('…'));
    assert!(find("residency").contains("warm") && find("residency").ends_with('…'));
    assert!(find("path").contains('…') && find("path").ends_with(".gguf"));
    assert!(
        texts(&lines)
            .iter()
            .any(|line| line.starts_with(" RECORD ─"))
    );
}

#[test]
fn the_compact_detail_skips_what_the_row_shows() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_bytes = Some(4 * (1 << 30));
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
    assert!(text(&pathless[3]).contains("4.3 GB"));

    let motion = settled();
    let full = full_lines(&record, &facts, false, 80, &Look::settled(&motion));
    assert!(!labels_of(&full).contains(&"runtime".to_owned()));
    assert_eq!(text(&full[2]), " no runtime · ollama · 4.3 GB");
    assert!(full.len() > STACKED_DETAIL_ROWS as usize);
    assert!(!texts(&full).iter().any(|line| line.starts_with(" RECORD")));
}

#[test]
fn the_size_row_names_the_disk_figure_when_it_differs() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_bytes = Some(34_000_000_000);
    record.serving_bytes = Some(8_500_000_000);
    record.context_length = Some(32_768);
    let line = text(&size_line(&record, 80));
    assert!(
        line.contains("8.5 GB · ctx 32k · 34 GB on disk"),
        "{line:?}"
    );
}

#[test]
fn the_size_row_has_no_disk_suffix_when_they_agree() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_bytes = Some(4_300_000_000);
    let line = text(&size_line(&record, 80));
    assert!(line.contains("4.3 GB"), "{line:?}");
    assert!(!line.contains("on disk"), "{line:?}");

    record.serving_bytes = Some(4_300_000_000);
    assert!(!text(&size_line(&record, 80)).contains("on disk"));
}

#[test]
fn the_title_escapes_the_name() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.alias = Some("evil\u{1b}[31mred bidi\u{202e}gpj".to_owned());
    assert_eq!(title(&record), " evil\\u{1b}[31mred bidi\\u{202e}gpj ");
}

#[test]
fn the_card_opens_with_the_name_its_facts_and_its_chips() {
    let mut record = record_with("qwen", vec![Capability::chat(), Capability::from("tools")]);
    record.footprint_bytes = Some(4_300_000_000);
    record.context_length = Some(32_768);
    let motion = settled();
    let lines = full_lines(
        &record,
        &facts_with_memory(64),
        false,
        60,
        &Look::settled(&motion),
    );
    assert_eq!(text(&lines[1]), " qwen");
    assert_eq!(lines[1].spans[1].style, BOLD);
    assert_eq!(text(&lines[2]), " no runtime · ollama · 4.3 GB · ctx 32k");
    assert_eq!(text(&lines[4]), "  chat   tools ");
    assert_eq!(lines[4].spans[1].style.bg, Some(RAISED));
    assert!(text(&lines[6]).starts_with(" MEMORY ─"));
}

#[test]
fn the_gauge_shows_what_warming_would_take_beside_what_is_loaded() {
    let mut record = record_with("m", vec![Capability::chat()]);
    record.footprint_bytes = Some(8 << 30);
    let facts = Facts {
        residents: vec![resident_with_bytes("other", Holder::Local, 16 << 30)],
        ..facts_with_memory(64)
    };
    let motion = settled();
    let look = Look::settled(&motion);
    let gauge = gauge_row(&record, &facts, 50, &look).expect("a gauge");
    let cells = 50 - GAUGE_SUFFIX;
    let bar: String = gauge.spans[1..4]
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(bar.chars().count(), cells);
    assert_eq!(
        gauge.spans[1].content.chars().count(),
        cells / 4,
        "a quarter is the other model"
    );
    assert_eq!(
        gauge.spans[2].content.chars().count(),
        cells / 8,
        "an eighth is this one"
    );
    assert!(text(&gauge).ends_with("if warmed"));
    assert!(gauge_row(&record_with("m", Vec::new()), &facts, 50, &look).is_none());
}
