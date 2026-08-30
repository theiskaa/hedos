use super::*;

use kernel::discovery::service::KindStat;
use kernel::records::SourceKind;

#[test]
fn a_scan_summary_speaks_the_strip_register() {
    let mut summary = DiscoverySummary {
        total_count: 12,
        issues: vec!["a".to_owned(), "b".to_owned()],
        ..DiscoverySummary::default()
    };
    summary.per_kind.insert(
        SourceKind::huggingface_cache(),
        KindStat { count: 9, bytes: 0 },
    );
    summary
        .per_kind
        .insert(SourceKind::ollama(), KindStat { count: 3, bytes: 0 });
    summary
        .per_kind
        .insert(SourceKind::lm_studio(), KindStat { count: 0, bytes: 0 });
    let line = scan_summary(&summary);
    assert!(line.starts_with("found 12 models · "));
    assert!(line.contains("3 ollama") && line.contains("9 hf"));
    assert!(line.ends_with(" · 2 issues"));
    assert!(!line.contains("lm studio"));
    assert!(!line.contains('\u{2014}') && !line.contains(", "));
    assert!(!line.ends_with('.'));
    assert!(line.chars().all(|c| !c.is_uppercase()));
    assert_eq!(scan_summary(&DiscoverySummary::default()), "found nothing");
    let one = DiscoverySummary {
        total_count: 1,
        ..DiscoverySummary::default()
    };
    assert_eq!(scan_summary(&one), "found 1 model");
}
