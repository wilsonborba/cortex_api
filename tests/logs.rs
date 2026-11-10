use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use cortex_api::{
    core::{
        config::Config,
        logs::{self, LineFormat, LogHub, Redactor},
    },
    presentation,
};
use futures_util::StreamExt;
use std::net::SocketAddr;
use tower::ServiceExt;
fn subscriber(config: &Config, hub: LogHub) -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .event_format(LineFormat {
            config: config.clone(),
            redactor: Redactor::new(vec!["FAKE_ENV_VALUE".into()]),
        })
        .with_writer(hub)
        .finish()
}
#[test]
fn exact_format_debug_switch_and_secret_fields() {
    for development in [false, true] {
        let config = Config {
            development,
            ..Default::default()
        };
        let hub = LogHub::new(&config);
        let mut rx = hub.subscribe();
        tracing::subscriber::with_default(subscriber(&config, hub), || {
            logs::debug("fixture", "Debug message");
            logs::info("fixture", "Info message");
            logs::warning("fixture", "Warning message");
            logs::error("fixture", "Error message");
            tracing::info!(target:"cortex_api::fixture",text="PRIVATE_USER_TEXT",model_output="PRIVATE_MODEL_TEXT",password="PRIVATE_PASSWORD",cookie="PRIVATE_COOKIE",input_tokens=42_u64,"Output completed");
            tracing::info!(target:"cortex_api::fixture","FAKE_ENV_VALUE password=FAKE_INLINE_VALUE Bearer FAKE_BEARER_VALUE postgres://user:FAKE_URL_VALUE@localhost/db");
            tracing::info!(target:"reqwest::fixture","PRIVATE_HTTP_DATA");
        });
        let mut lines = vec![];
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        assert_eq!(lines.len(), if development { 6 } else { 5 });
        for line in &lines {
            let fields: Vec<_> = line.splitn(5, ' ').collect();
            assert_eq!(fields.len(), 5);
            chrono::DateTime::parse_from_rfc3339(fields[0]).unwrap();
            assert!(matches!(fields[1], "DEBUG" | "INFO" | "WARNING" | "ERROR"));
            assert_eq!(fields[2], "cortex");
            assert!(!fields[3].is_empty());
            assert!(!line.contains('\n'));
        }
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("INFO cortex fixture Info message"))
        );
        let all = lines.join("\n");
        assert_eq!(all.contains("DEBUG"), development);
        assert!(all.contains("input_tokens=42"));
        for private in [
            "PRIVATE_USER_TEXT",
            "PRIVATE_MODEL_TEXT",
            "PRIVATE_PASSWORD",
            "PRIVATE_COOKIE",
            "PRIVATE_HTTP_DATA",
            "FAKE_ENV_VALUE",
            "FAKE_INLINE_VALUE",
            "FAKE_BEARER_VALUE",
            "FAKE_URL_VALUE",
        ] {
            assert!(!all.contains(private));
        }
    }
}
#[tokio::test]
async fn stream_is_lan_only_and_uses_the_same_redacted_line() {
    let config = Config::default();
    let hub = LogHub::new(&config);
    let router = presentation::with_response_contract(presentation::logs::router(hub.clone()));
    for peer in [
        None,
        Some("203.0.113.1:5000".parse::<SocketAddr>().unwrap()),
    ] {
        let mut request = Request::get("/logs/stream")
            .header("x-forwarded-for", "127.0.0.1")
            .header("x-access-level", "admin")
            .body(Body::empty())
            .unwrap();
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        assert_eq!(
            router.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut request = Request::get("/logs/stream").body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let mut journal = hub.subscribe();
    tracing::subscriber::with_default(subscriber(&config, hub), || {
        logs::info("fixture", "Live message")
    });
    let line = journal.recv().await.unwrap();
    let mut body = response.into_body().into_data_stream();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame = String::from_utf8(frame.to_vec()).unwrap();
    assert!(frame.contains("event: log"));
    assert!(frame.contains(&line));
}
