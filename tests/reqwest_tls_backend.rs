const NON_REDDIT_CLIENTS: &[(&str, &str)] = &[
    ("search", include_str!("../src/dal/remote/search.rs")),
    ("openalex", include_str!("../src/dal/remote/openalex.rs")),
    (
        "file_worker",
        include_str!("../src/dal/remote/file_worker.rs"),
    ),
    ("web_page", include_str!("../src/dal/remote/web_page.rs")),
    ("ollama", include_str!("../src/dal/remote/ollama.rs")),
    ("runs", include_str!("../src/dal/remote/runs.rs")),
    (
        "hippocampus",
        include_str!("../src/dal/remote/hippocampus.rs"),
    ),
    ("loyalty", include_str!("../src/dal/remote/loyalty.rs")),
];

fn client_builder(source: &str) -> &str {
    let start = source
        .find("reqwest::Client::builder()")
        .expect("client builder exists");
    let rest = &source[start..];
    let end = rest.find(".build()").expect("client builder is built") + ".build()".len();
    &rest[..end]
}

#[test]
fn non_reddit_clients_explicitly_select_rustls() {
    for (name, source) in NON_REDDIT_CLIENTS {
        assert!(
            client_builder(source).contains(".use_rustls_tls()"),
            "{name} must explicitly select rustls"
        );
    }
}

#[test]
fn reddit_client_explicitly_selects_native_tls() {
    let source = include_str!("../src/dal/remote/reddit.rs");
    assert!(
        client_builder(source).contains(".use_native_tls()"),
        "Reddit must explicitly select native TLS"
    );
}
