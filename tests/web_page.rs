use axum::{
    Router,
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use cortex_api::{
    core::config::Config,
    dal::remote::web_page::{PageReader, allowed_address, page_url},
    domain::{
        models::{context::ContextText, web_page::PageFailure},
        services::chunks::{pasted_urls, rank, split},
    },
};
use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
fn article() -> String {
    format!(
        "<html><head><title>Rust article</title><script src='/asset.js'></script></head><body><nav>Menu</nav><article><h1>Rust article</h1><p>{}</p><p>{}</p><a href='/related#part'>Related article</a><img src='/image.png'><script>scriptnoise</script></article></body></html>",
        "Rust ownership protects memory safety. ".repeat(30),
        "The compiler checks lifetimes and borrowing. ".repeat(30)
    )
}
#[test]
fn split_rank_pasted_urls_and_forbidden_addresses_need_no_network() {
    let config = Config::default();
    assert_eq!(config.web_page_timeout, Duration::from_secs(8));
    assert_eq!(config.web_parallel_pages, 5);
    let chunks = split(&config, &"ภาษาไทย 😀 Rust memory safety. ".repeat(100));
    assert!(chunks.len() > 1);
    assert!(chunks.iter().all(|c| config.estimate_tokens(c) <= 500));
    let texts = [
        "The apple orchard grows fruit.",
        "Rust ownership and borrowing protect memory safety.",
        "There is rain over the sea.",
    ];
    let ranked = rank(
        "Rust memory safety",
        texts
            .into_iter()
            .map(|s| ContextText {
                label: "test".into(),
                source: None,
                text: s.into(),
            })
            .collect(),
    );
    assert!(ranked[0].chunk.text.contains("Rust ownership"));
    assert!(ranked[0].score > ranked[1].score);
    assert_eq!(
        pasted_urls(
            "Read https://example.org/page. Then (https://en.wikipedia.org/wiki/Rust_(programming_language)). Again https://example.org/page#part"
        ),
        [
            "https://example.org/page",
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        ]
    );
    for target in [
        "http://192.168.1.104/",
        "http://[::ffff:192.168.1.104]/",
        "file:///etc/passwd",
        "https://user:password@example.org/",
    ] {
        assert!(page_url(target).is_err());
    }
    assert!(!allowed_address("192.168.1.104".parse::<IpAddr>().unwrap()));
    assert!(!allowed_address(
        "::ffff:192.168.1.104".parse::<IpAddr>().unwrap()
    ));
    assert!(allowed_address("127.0.0.1".parse().unwrap()));
}
#[tokio::test]
async fn one_get_main_text_no_assets_type_size_timeout_and_redirect() {
    let calls = Arc::new(AtomicUsize::new(0));
    let hits = calls.clone();
    let app = Router::new()
        .route(
            "/page",
            get(move |headers: HeaderMap| {
                assert_eq!(headers["user-agent"], "AsodyaCortex/2");
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    ([("content-type", "text/html; charset=utf-8")], article())
                }
            }),
        )
        .route(
            "/wrong",
            get(|| async { ([("content-type", "application/pdf")], "not HTML") }),
        )
        .route(
            "/large",
            get(|| async {
                (
                    [("content-type", "text/html")],
                    "x".repeat(2 * 1024 * 1024 + 1),
                )
            }),
        )
        .route(
            "/chunked",
            get(|| async {
                Response::builder()
                    .header("content-type", "text/html")
                    .body(Body::from_stream(futures_util::stream::iter((0..3).map(
                        |_| Ok::<_, std::convert::Infallible>(vec![b'x'; 1024 * 1024]),
                    ))))
                    .unwrap()
            }),
        )
        .route(
            "/stall",
            get(|| async { std::future::pending::<Response>().await }),
        )
        .route(
            "/redirect",
            get(|| async {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", "http://192.168.1.104/")],
                )
                    .into_response()
            }),
        )
        .fallback(|| async {
            panic!("An asset or extra URL was fetched");
            #[allow(unreachable_code)]
            StatusCode::NOT_FOUND
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{address}");
    let config = Config {
        web_page_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let reader = PageReader::new(&config).unwrap();
    let page = reader.read(&format!("{base}/page")).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(page.source.title, "Rust article");
    assert!(page.chunks.iter().any(|c| c.text.contains("ownership")));
    assert!(page.chunks.iter().all(|c| !c.text.contains("scriptnoise")));
    assert!(
        page.links
            .iter()
            .any(|l| l.url == format!("{base}/related"))
    );
    for (path, expected) in [
        ("wrong", PageFailure::WrongType),
        ("large", PageFailure::TooLarge),
        ("chunked", PageFailure::TooLarge),
        ("stall", PageFailure::Timeout),
        ("redirect", PageFailure::Unavailable),
    ] {
        let result = reader.read(&format!("{base}/{path}")).await;
        assert!(
            matches!(result,Err(error) if error==expected),
            "wrong failure for {path}"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}
#[tokio::test]
async fn shared_reader_allows_at_most_five_parallel_gets() {
    use tokio::sync::Semaphore;
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let gate = Arc::new(Semaphore::new(0));
    let (a, m, e, g) = (
        active.clone(),
        maximum.clone(),
        entered.clone(),
        gate.clone(),
    );
    let app = Router::new().route(
        "/page",
        get(move || {
            let (a, m, e, g) = (a.clone(), m.clone(), e.clone(), g.clone());
            async move {
                let count = a.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(count, Ordering::SeqCst);
                e.add_permits(1);
                g.acquire().await.unwrap().forget();
                a.fetch_sub(1, Ordering::SeqCst);
                ([("content-type", "text/html")], article())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let reader = PageReader::new(&Config::default()).unwrap();
    let requests = tokio::spawn(async move {
        futures_util::future::join_all((0..10).map(|_| reader.read(&url))).await
    });
    entered.acquire_many(5).await.unwrap().forget();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(active.load(Ordering::SeqCst), 5);
    gate.add_permits(10);
    let results = requests.await.unwrap();
    assert!(results.iter().all(Result::is_ok));
    assert_eq!(maximum.load(Ordering::SeqCst), 5);
    server.abort();
}
