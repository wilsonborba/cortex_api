use cortex_api::dal::local::research_sources::ResearchSource;
use cortex_api::domain::models::run_record::WebMode;
use cortex_api::domain::services::research_router::{
    EvidenceItem, EvidenceLedger, EvidenceStage, ResearchBudget, ResearchPurpose,
    ResearchRouteRequest, canonicalize_url, select_sources,
};

fn src(host: &str, subjects: &[&str], langs: &str, regions: &str, mode: &str, desc: &str) -> ResearchSource {
    ResearchSource {
        host: host.into(),
        name: host.into(),
        url: format!("https://{host}"),
        description: desc.into(),
        languages: langs.into(),
        regions: regions.into(),
        access_note: String::new(),
        access_mode: mode.into(),
        is_enabled: mode != "unavailable",
        min_spacing_ms: 500,
        max_requests_per_run: 5,
        subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[test]
fn selects_relevant_subset_never_sweeps_all_200_sites() {
    let mut catalog = Vec::new();
    for i in 0..200 {
        let sub = if i < 5 { "ai-ml" } else if i < 10 { "climate" } else { "sports" };
        catalog.push(src(
            &format!("site-{i}.example"),
            &[sub],
            if i == 0 { "th,en" } else { "en" },
            if i == 0 { "th" } else { "global" },
            if i == 2 { "unavailable" } else { "public_api" },
            "academic paper journal",
        ));
    }

    let req = ResearchRouteRequest {
        subjects: vec!["ai-ml".into(), "climate".into()],
        purpose: ResearchPurpose::Academic,
        language: Some("th".into()),
        region: Some("th".into()),
        mode: WebMode::Medium,
        budget: ResearchBudget::for_mode(WebMode::Medium),
    };

    let plan = select_sources(&catalog, &req);
    assert_eq!(plan.catalog_total, 200);
    assert!(plan.selected.len() <= 6, "must select small subset, got {}", plan.selected.len());
    assert!(!plan.selected.is_empty());
    assert!(plan.selected.iter().all(|r| r.source.host != "site-2.example"));
    assert_eq!(plan.selected[0].source.host, "site-0.example");
}

#[test]
fn budget_downgrades_deep_mode_when_time_or_requests_are_low() {
    let catalog = vec![
        src("a.example", &["science"], "en", "global", "public_api", "journal"),
        src("b.example", &["science"], "en", "global", "public_api", "journal"),
        src("c.example", &["science"], "en", "global", "public_api", "journal"),
        src("d.example", &["science"], "en", "global", "public_api", "journal"),
    ];
    let req = ResearchRouteRequest {
        subjects: vec!["science".into()],
        purpose: ResearchPurpose::Academic,
        language: None,
        region: None,
        mode: WebMode::Deep,
        budget: ResearchBudget {
            remaining_requests: 5,
            remaining_pages: 3,
            remaining_time_ms: 5_000,
            max_sources: 12,
        },
    };
    let plan = select_sources(&catalog, &req);
    assert_eq!(plan.effective_mode, WebMode::Fast);
    assert!(plan.selected.len() <= 3);
    assert_eq!(plan.page_budget, 3);
}

#[test]
fn deduplicates_canonical_urls_and_publication_ids_and_filters_citations() {
    assert_eq!(
        canonicalize_url("HTTPS://Example.COM/paper/?utm_source=x&id=42#top"),
        "https://example.com/paper?id=42"
    );

    let mut ledger = EvidenceLedger::default();
    ledger.record(EvidenceItem {
        canonical_url: "https://openalex.org/W123/?utm_medium=api".into(),
        publication_id: Some("DOI:10.1000/abc".into()),
        title: String::new(),
        source_host: "openalex.org".into(),
        stage: EvidenceStage::MetadataOnly,
    });
    ledger.record(EvidenceItem {
        canonical_url: "https://publisher.example/papers/abc".into(),
        publication_id: Some("doi:10.1000/abc".into()),
        title: "Paper Title".into(),
        source_host: "publisher.example".into(),
        stage: EvidenceStage::ReadContent,
    });
    ledger.record(EvidenceItem {
        canonical_url: "https://other.example/link-only".into(),
        publication_id: None,
        title: "Unread Link".into(),
        source_host: "other.example".into(),
        stage: EvidenceStage::FoundLink,
    });

    assert_eq!(ledger.all_items().len(), 2);
    let citable = ledger.citable_items();
    assert_eq!(citable.len(), 1);
    assert_eq!(citable[0].title, "Paper Title");
    assert_eq!(citable[0].stage.label(), "read_content");
}
