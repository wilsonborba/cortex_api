use std::sync::Arc;

use cortex_api::domain::services::research_limits::{
    ConditionalCacheMeta, FetchDecision, InMemoryLimitStore, PolitenessConfig,
    ResearchPolitenessGuard, parse_bounded_retry_after,
};

#[tokio::test]
async fn shared_quota_limits_across_users_and_429_cooldown() {
    let store = Arc::new(InMemoryLimitStore::default());
    let guard = ResearchPolitenessGuard::new(
        PolitenessConfig {
            max_global_concurrency: 2,
            default_host_spacing_ms: 10,
            request_timeout_ms: 5_000,
            max_response_bytes: 1024 * 1024,
            max_backoff_secs: 20,
        },
        store,
    );

    // Account/provider scope quota of 2 requests
    let p1 = guard
        .acquire_host("api.openalex.org", 10, "provider:openalex", 2, 60)
        .await
        .unwrap();
    assert!(p1.is_ok());
    drop(p1);

    let p2 = guard
        .acquire_host("api.openalex.org", 10, "provider:openalex", 2, 60)
        .await
        .unwrap();
    assert!(p2.is_ok());
    drop(p2);

    // 3rd request from another user in same provider scope hits shared quota
    let p3 = guard
        .acquire_host("api.openalex.org", 10, "provider:openalex", 2, 60)
        .await
        .unwrap();
    assert!(matches!(p3, Err(FetchDecision::RateLimitedCooldown { .. })));

    // 429 honors Retry-After bounded by max_backoff_secs
    assert_eq!(parse_bounded_retry_after(Some("120"), 20), Some(20));
    let d429 = guard
        .handle_status("example.com", 429, Some("12"), false)
        .await
        .unwrap();
    assert_eq!(d429, FetchDecision::RateLimitedCooldown { wait_secs: 12 });

    // Cooldown prevents subsequent acquire_host
    let blocked = guard
        .acquire_host("example.com", 10, "host:example.com", 10, 60)
        .await
        .unwrap();
    assert!(matches!(blocked, Err(FetchDecision::RateLimitedCooldown { .. })));
}

#[tokio::test]
async fn stops_cleanly_on_401_403_or_challenge_without_retry() {
    let store = Arc::new(InMemoryLimitStore::default());
    let guard = ResearchPolitenessGuard::new(PolitenessConfig::default(), store);

    let d403 = guard
        .handle_status("gated.example", 403, None, false)
        .await
        .unwrap();
    assert!(matches!(d403, FetchDecision::BlockedStop { .. }));

    let dchal = guard
        .handle_status("cf.example", 200, None, true)
        .await
        .unwrap();
    assert!(matches!(dchal, FetchDecision::BlockedStop { .. }));

    let meta = ConditionalCacheMeta {
        etag: Some("\"v1\"".into()),
        last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
    };
    let hdrs = meta.request_headers();
    assert_eq!(hdrs.len(), 2);
}
