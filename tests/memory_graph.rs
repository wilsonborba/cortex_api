use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query, State},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use cortex_api::{
    core::config::Config,
    dal::remote::hippocampus::Hippocampus,
    domain::services::memory_graph::MemoryGraph,
    presentation::{memory_graph::router, with_response_contract},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;
type Calls = Arc<Mutex<Vec<(Option<String>, HashMap<String, String>)>>>;
fn memory(id: &str, user: &str) -> Value {
    json!({"id":id,"node_type":"memory","label":id,"metadata":{"id":id,"workspace_id":"cortex","app_id":"cortex","user_id":user}})
}
/// Hippocampus double: alice owns m-a, bob owns m-b. A query with
/// `user_id=forged` answers with bob's node to test the scope check.
fn answer(user: &str, node: Option<&str>) -> Response {
    let owned = if user == "forged" { "bob" } else { user };
    let own = format!("m-{}", &owned[..1]);
    let nodes = vec![
        memory(&own, owned),
        json!({"id":"tag:t1","node_type":"tag","label":"shared","metadata":{}}),
    ];
    match node {
        Some(n) if n != own && n != "tag:t1" => (
            StatusCode::NOT_FOUND,
            Json(json!({"detail":"memory not found"})),
        )
            .into_response(),
        Some(n) => Json(json!({"data":{"nodes":nodes,"edges":[{"source_id":own,"target_id":"tag:t1","edge_type":"tagged_with"}],"truncated":false,"node":{"id":n}}})).into_response(),
        None => Json(json!({"data":{"nodes":nodes,"edges":[],"truncated":false}})).into_response(),
    }
}
async fn server() -> (Calls, Config) {
    let calls: Calls = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        hippocampus_url: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let app = Router::new()
        .route(
            "/api/v1/graph",
            get(
                |State(c): State<Calls>, Query(q): Query<HashMap<String, String>>| async move {
                    let user = q.get("user_id").cloned().unwrap_or_default();
                    c.lock().unwrap().push((None, q));
                    answer(&user, None)
                },
            ),
        )
        .route(
            "/api/v1/graph/nodes/{id}",
            get(
                |State(c): State<Calls>,
                 Path(id): Path<String>,
                 Query(q): Query<HashMap<String, String>>| async move {
                    let user = q.get("user_id").cloned().unwrap_or_default();
                    c.lock().unwrap().push((Some(id.clone()), q));
                    answer(&user, Some(&id))
                },
            ),
        )
        .with_state(calls.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (calls, config)
}
async fn call(config: &Config, path: &str, user: Option<&str>) -> (StatusCode, Value) {
    let app = with_response_contract(router(MemoryGraph::new(
        Hippocampus::new(config).unwrap(),
        config.clone(),
    )));
    let mut req = Request::get(path).extension(axum::extract::ConnectInfo(
        "203.0.113.1:1234".parse::<std::net::SocketAddr>().unwrap(),
    ));
    if let Some(user) = user {
        req = req.header("x-uuid", user);
    }
    let response = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    (
        response.status(),
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap(),
    )
}
#[tokio::test]
async fn scope_sent_in_one_call_and_depth_limits() {
    let (calls, config) = server().await;
    assert_eq!(config.memory_graph_max_depth, 3);
    let (status, body) = call(
        &config,
        "/memory-graph?app_id=cortex&limit=50&depth=3",
        Some("alice"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "Memory graph loaded");
    assert_eq!(body["data"]["nodes"][0]["id"], "m-a");
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (node, q) = &calls[0];
        assert!(node.is_none());
        assert_eq!(q["workspace_id"], "cortex");
        assert_eq!(q["app_id"], "cortex");
        assert_eq!(q["user_id"], "alice");
        assert_eq!(q["limit"], "50");
        assert_eq!(q["depth"], "3");
    }
    for depth in ["1", "2", "3"] {
        let (status, _) = call(
            &config,
            &format!("/memory-graph?app_id=cortex&depth={depth}"),
            Some("alice"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "depth {depth}");
    }
    let before = calls.lock().unwrap().len();
    for query in ["depth=0", "depth=4", "limit=0", "depth=x"] {
        let (status, _) = call(
            &config,
            &format!("/memory-graph?app_id=cortex&{query}"),
            Some("alice"),
        )
        .await;
        assert!(status.is_client_error(), "{query}");
    }
    let (status, body) = call(&config, "/memory-graph?app_id=unknown", Some("alice")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["error_number"], 1009);
    let (status, body) = call(&config, "/memory-graph?app_id=cortex", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["error_number"], 1005);
    assert_eq!(calls.lock().unwrap().len(), before);
}
#[tokio::test]
async fn node_context_in_scope_and_other_user_node_refused() {
    let (calls, config) = server().await;
    let (status, body) = call(
        &config,
        "/memory-graph/nodes/tag:t1?app_id=cortex",
        Some("alice"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["node"]["id"], "tag:t1");
    assert_eq!(calls.lock().unwrap()[0].0.as_deref(), Some("tag:t1"));
    assert_eq!(calls.lock().unwrap()[0].1["user_id"], "alice");
    let (status, body) = call(
        &config,
        "/memory-graph/nodes/m-b?app_id=cortex",
        Some("alice"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["data"]["error_number"], 1039);
    // A wrong-scope answer from Hippocampus is never passed on.
    let (status, body) = call(&config, "/memory-graph?app_id=cortex", Some("forged")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["data"]["error_number"], 1029);
    assert!(!body.to_string().contains("m-b"));
}
#[tokio::test]
async fn hippocampus_down_is_a_load_failure() {
    let config = Config {
        hippocampus_url: "http://127.0.0.1:1".into(),
        ..Default::default()
    };
    let (status, body) = call(&config, "/memory-graph?app_id=cortex", Some("alice")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["data"]["error_number"], 1029);
}
