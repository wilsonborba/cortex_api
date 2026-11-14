//! Memory overview (#121): rules on a synthetic scenario, the routes with a
//! Hippocampus double, and (ignored by default) the SQL scope on the test
//! database. All data is synthetic.
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Query},
    http::{Request, StatusCode},
    routing::get,
};
use chrono::{DateTime, Duration, TimeZone, Utc};
use cortex_api::{
    core::config::Config,
    dal::{
        local::{
            database::{self, Target},
            memory_records::{MemoryRecord, MemoryRecords},
        },
        remote::{cache::RedisCache, hippocampus::Hippocampus},
    },
    domain::{
        models::memory_overview::{ItemsQuery, Period, SourceFilter},
        services::{memory_graph::MemoryGraph, memory_overview::Index},
    },
    presentation::{memory_graph::router, with_response_contract},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use tower::ServiceExt;
use uuid::Uuid;

const CHAT_A: &str = "0190f000-0000-7000-8000-00000000000a";
const CHAT_B: &str = "0190f000-0000-7000-8000-00000000000b";
const CHAT_C: &str = "0190f000-0000-7000-8000-00000000000c";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap()
}

fn memory(id: &str, user: &str, title: &str, kind: &str, at: DateTime<Utc>) -> Value {
    json!({"id":id,"node_type":"memory","label":title,"metadata":{
        "id":id,"workspace_id":"cortex","app_id":"cortex","user_id":user,
        "title":title,"summary":"","content":format!("Synthetic content of {title}"),
        "memory_type":kind,"status":"active","created_at":at.to_rfc3339(),
        "updated_at":at.to_rfc3339()}})
}

/// The scenario of the issue for one user: 3 chats, 2 files, 1 person,
/// 4 topics, 13 memories with the title "ola" in one chat, and a memory
/// used in 2 answers. 19 memories in total.
fn graph(user: &str) -> Value {
    if user != "alice" {
        return json!({"nodes":[],"edges":[],"truncated":false});
    }
    let t = now();
    let mut nodes = vec![];
    for i in 0..13 {
        let title = if i % 2 == 0 { "ola" } else { " OLA  " };
        nodes.push(memory(
            &format!("m-ola-{i:02}"),
            user,
            title,
            "episodic",
            t - Duration::minutes(i),
        ));
    }
    nodes.push(memory(
        "m-read",
        user,
        "Reading plan",
        "episodic",
        t - Duration::days(2),
    ));
    nodes.push(memory(
        "m-tax",
        user,
        "Tax notes",
        "episodic",
        t - Duration::days(10),
    ));
    nodes.push(memory(
        "m-file-1",
        user,
        "report.pdf",
        "file",
        t - Duration::days(40),
    ));
    nodes.push(memory(
        "m-file-2",
        user,
        "notes.txt",
        "semantic",
        t - Duration::days(41),
    ));
    nodes.push(memory(
        "m-note",
        user,
        "Saved idea",
        "semantic",
        t - Duration::days(3),
    ));
    // A note with the same title as the chat memories: a different source.
    nodes.push(memory(
        "m-ola-note",
        user,
        "ola",
        "semantic",
        t - Duration::hours(2),
    ));
    for (id, kind, label) in [
        ("tag:t1", "tag", "project:cortex"),
        ("tag:t2", "tag", "general:reading"),
        ("tag:t3", "tag", "work_area:taxes"),
        ("tag:t4", "tag", "topic:ola-greeting"),
        ("entity:p1", "entity", "Example Person"),
        ("resource:f1", "file", "report.pdf"),
        ("resource:c1", "chat", "Deploy problems"),
    ] {
        nodes.push(json!({"id":id,"node_type":kind,"label":label,"metadata":{}}));
    }
    let mut edges = vec![
        json!({"source_id":"m-read","target_id":"tag:t2","edge_type":"tagged_with"}),
        json!({"source_id":"m-read","target_id":"entity:p1","edge_type":"mentions"}),
        json!({"source_id":"m-tax","target_id":"tag:t3","edge_type":"tagged_with"}),
        json!({"source_id":"m-tax","target_id":"entity:p1","edge_type":"mentions"}),
        json!({"source_id":"m-note","target_id":"tag:t1","edge_type":"tagged_with"}),
        json!({"source_id":"m-note","target_id":"m-tax","edge_type":"supersedes"}),
        json!({"source_id":"m-file-1","target_id":"resource:f1","edge_type":"attached"}),
        json!({"source_id":"m-file-1","target_id":"tag:t1","edge_type":"tagged_with"}),
    ];
    for i in 0..13 {
        edges.push(json!({"source_id":format!("m-ola-{i:02}"),"target_id":"tag:t4","edge_type":"tagged_with"}));
        edges.push(json!({"source_id":format!("m-ola-{i:02}"),"target_id":"resource:c1","edge_type":"created_in"}));
    }
    json!({"nodes":nodes,"edges":edges,"truncated":false})
}

fn record(
    kind: &str,
    memory: &str,
    ref_id: &str,
    title: &str,
    session: &str,
    at: DateTime<Utc>,
    output: Option<&str>,
) -> MemoryRecord {
    MemoryRecord {
        kind: kind.into(),
        memory_id: memory.into(),
        ref_id: ref_id.into(),
        ref_title: title.into(),
        session_id: session.into(),
        at,
        output_id: output.map(str::to_owned),
    }
}

/// The SQL rows of the same scenario.
fn records() -> Vec<MemoryRecord> {
    let t = now();
    let mut rows = vec![];
    for i in 0..13 {
        rows.push(record(
            "chat",
            &format!("m-ola-{i:02}"),
            CHAT_A,
            "Deploy problems",
            CHAT_A,
            t - Duration::minutes(i),
            Some("o"),
        ));
    }
    rows.push(record(
        "chat",
        "m-read",
        CHAT_B,
        "Reading",
        CHAT_B,
        t - Duration::days(2),
        Some("o"),
    ));
    rows.push(record(
        "chat",
        "m-tax",
        CHAT_C,
        "Taxes",
        CHAT_C,
        t - Duration::days(10),
        Some("o"),
    ));
    rows.push(record(
        "file",
        "m-file-2",
        "file-2",
        "Reading",
        CHAT_B,
        t - Duration::days(41),
        None,
    ));
    rows.push(record(
        "used",
        "m-read",
        CHAT_C,
        "Taxes",
        CHAT_C,
        t - Duration::days(1),
        Some("out-1"),
    ));
    rows.push(record(
        "used",
        "m-read",
        CHAT_C,
        "Taxes",
        CHAT_C,
        t - Duration::hours(3),
        Some("out-2"),
    ));
    rows
}

fn query(value: Value) -> ItemsQuery {
    let mut value = value;
    value["app_id"] = json!("cortex");
    serde_json::from_value(value).unwrap()
}

#[test]
fn overview_numbers_and_facets() {
    let index = Index::build(&graph("alice"), &records());
    let o = index.overview(now());
    let s = &o["summary"];
    assert_eq!(s["memories"], 19);
    assert_eq!(s["chats"], 3);
    assert_eq!(s["files"], 2);
    assert_eq!(s["people"], 1);
    assert_eq!(s["topics"], 4);
    assert_eq!(s["notes"], 2);
    assert_eq!(s["used_7_days"], 1);
    assert_eq!(s["added_7_days"], 16);
    let size: usize = (0..13)
        .map(|i| {
            format!(
                "Synthetic content of {}",
                if i % 2 == 0 { "ola" } else { " OLA  " }
            )
            .len()
        })
        .sum::<usize>()
        + [
            "Reading plan",
            "Tax notes",
            "report.pdf",
            "notes.txt",
            "Saved idea",
            "ola",
        ]
        .iter()
        .map(|t| format!("Synthetic content of {t}").len())
        .sum::<usize>();
    assert_eq!(s["size_bytes"], size);
    assert_eq!(s["last_used_at"], json!(now() - Duration::hours(3)));
    let sources = &o["facets"]["sources"];
    assert_eq!(
        sources,
        &json!([
            {"kind":"chat","label":"Chats","count":15},
            {"kind":"file","label":"Files","count":2},
            {"kind":"note","label":"Saved notes","count":2},
        ])
    );
    let months = o["facets"]["months"].as_array().unwrap();
    assert_eq!(months.len(), 12);
    assert_eq!(months[11]["month"], "2026-11");
    assert_eq!(months[0]["month"], "2025-12");
    assert_eq!(
        months
            .iter()
            .map(|m| m["count"].as_u64().unwrap())
            .sum::<u64>(),
        19
    );
    // Topics: plain labels, top by count, ids never used as labels.
    let topics = o["facets"]["topics"].as_array().unwrap();
    assert_eq!(
        topics[0],
        json!({"id":"tag:t4","label":"Topic: ola greeting","count":13})
    );
    let labels: Vec<_> = topics
        .iter()
        .map(|t| t["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"reading"));
    assert!(labels.contains(&"Work area: taxes"));
    assert_eq!(
        o["facets"]["people"],
        json!([{"id":"entity:p1","label":"Example Person","count":2}])
    );
    assert_eq!(
        o["facets"]["types"][0],
        json!({"type":"episodic","label":"Conversation","count":15})
    );
}

#[test]
fn thirteen_ola_merge_into_one_item_but_not_across_sources() {
    let index = Index::build(&graph("alice"), &records());
    let page = index.items(&query(json!({})), now()).unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(page["total"], 19);
    // 13 chat "ola" memories become one row; the note "ola" stays apart.
    assert_eq!(items.len(), 7);
    let ola: Vec<_> = items
        .iter()
        .filter(|i| {
            i["title"]
                .as_str()
                .unwrap()
                .trim()
                .eq_ignore_ascii_case("ola")
        })
        .collect();
    assert_eq!(ola.len(), 2);
    let chat = ola.iter().find(|i| i["source"]["kind"] == "chat").unwrap();
    assert_eq!(chat["stack"]["count"], 13);
    assert_eq!(chat["id"], "m-ola-00");
    assert_eq!(
        chat["source"],
        json!({"kind":"chat","id":CHAT_A,"title":"Deploy problems"})
    );
    assert_eq!(chat["stack"]["ids"].as_array().unwrap().len(), 13);
    let note = ola.iter().find(|i| i["source"]["kind"] == "note").unwrap();
    assert!(note["stack"].is_null());
    // No two visible rows have the same title and source.
    let mut seen = std::collections::HashSet::new();
    for i in items {
        let key = (
            i["title"].as_str().unwrap().trim().to_lowercase(),
            i["source"].to_string(),
        );
        assert!(seen.insert(key));
    }
    let read = items.iter().find(|i| i["id"] == "m-read").unwrap();
    assert_eq!(read["used_in_answers"], 2);
    assert_eq!(read["last_used_at"], json!(now() - Duration::hours(3)));
    assert_eq!(read["topics"], json!([{"id":"tag:t2","label":"reading"}]));
    assert_eq!(
        read["people"],
        json!([{"id":"entity:p1","label":"Example Person"}])
    );
    let file = items.iter().find(|i| i["id"] == "m-file-1").unwrap();
    assert_eq!(
        file["source"],
        json!({"kind":"file","id":"resource:f1","title":"report.pdf"})
    );
    assert_eq!(
        file["files"],
        json!([{"id":"resource:f1","name":"report.pdf"}])
    );
    let file2 = items.iter().find(|i| i["id"] == "m-file-2").unwrap();
    assert_eq!(file2["source"]["kind"], "file");
    assert_eq!(file2["source"]["id"], "file-2");
}

#[test]
fn paging_filters_and_groups() {
    let index = Index::build(&graph("alice"), &records());
    // Paging: 2 items per page; the merged stack counts as one item.
    let first = index.items(&query(json!({"limit":2})), now()).unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    assert_eq!(first["items"][0]["stack"]["count"], 13);
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let mut ids: Vec<String> = vec![];
    let mut next = Some(cursor);
    let mut pages = 1;
    while let Some(c) = next {
        let page = index
            .items(&query(json!({"limit":2,"cursor":c})), now())
            .unwrap();
        for i in page["items"].as_array().unwrap() {
            ids.push(i["id"].as_str().unwrap().into());
        }
        next = page["next_cursor"].as_str().map(str::to_owned);
        pages += 1;
    }
    assert_eq!(pages, 4);
    assert_eq!(ids.len(), 5);
    assert!(!ids.iter().any(|i| i.starts_with("m-ola-0")));
    assert!(
        index
            .items(&query(json!({"cursor":"bad cursor"})), now())
            .is_err()
    );
    assert!(index.items(&query(json!({"limit":0})), now()).is_err());
    assert!(index.items(&query(json!({"limit":101})), now()).is_err());
    // Period and search.
    let week = index.items(&query(json!({"period":"7d"})), now()).unwrap();
    assert_eq!(week["total"], 16);
    let today = index
        .items(&query(json!({"period":"today"})), now())
        .unwrap();
    assert_eq!(today["total"], 14);
    let q = index.items(&query(json!({"q":"TAX"})), now()).unwrap();
    assert_eq!(q["total"], 1);
    let by_label = index
        .items(&query(json!({"q":"example person"})), now())
        .unwrap();
    assert_eq!(by_label["total"], 2);
    // Groups by chat, newest first; then the items of one group.
    let groups = index
        .items(&query(json!({"source":"chat"})), now())
        .unwrap();
    assert!(groups["items"].as_array().unwrap().is_empty());
    assert_eq!(
        groups["groups"][0],
        json!({"id":CHAT_A,"kind":"chat","title":"Deploy problems","count":13,
        "first_at":now() - Duration::minutes(12),"last_at":now()})
    );
    assert_eq!(groups["groups"].as_array().unwrap().len(), 3);
    let chat_a = index
        .items(&query(json!({"source":"chat","group_id":CHAT_A})), now())
        .unwrap();
    assert_eq!(chat_a["total"], 13);
    assert_eq!(chat_a["items"].as_array().unwrap().len(), 1);
    let person = index
        .items(
            &query(json!({"source":"person","group_id":"entity:p1"})),
            now(),
        )
        .unwrap();
    assert_eq!(person["total"], 2);
    let topics = index
        .items(&query(json!({"source":"topic"})), now())
        .unwrap();
    assert_eq!(topics["groups"].as_array().unwrap().len(), 4);
    let notes = index
        .items(&query(json!({"source":"note"})), now())
        .unwrap();
    assert_eq!(notes["total"], 2);
    assert!(notes["groups"].as_array().unwrap().is_empty());
    let q: ItemsQuery = query(json!({}));
    assert_eq!((q.source, q.period), (SourceFilter::All, Period::All));
}

#[test]
fn detail_connections_usage_and_tiny_map() {
    let index = Index::build(&graph("alice"), &records());
    let note = index.detail("m-note").unwrap();
    assert_eq!(note["content"], "Synthetic content of Saved idea");
    assert_eq!(
        note["connections"],
        json!([
            {"kind":"topic","target":{"id":"tag:t1","label":"Project: cortex","node_type":"tag"}},
            {"kind":"relation","relation_type":"supersedes","target":{"id":"m-tax","label":"Tax notes","node_type":"memory"}},
        ])
    );
    let read = index.detail("m-read").unwrap();
    assert_eq!(
        read["connections"][0],
        json!({"kind":"source_chat","target":{"id":CHAT_B,"label":"Reading","node_type":"chat"}})
    );
    assert_eq!(read["connections"][1]["kind"], "person");
    assert_eq!(read["connections"][2]["kind"], "topic");
    assert_eq!(read["usage"].as_array().unwrap().len(), 2);
    assert_eq!(read["usage"][0]["output_id"], "out-2");
    let ola = index.detail("m-ola-03").unwrap();
    let last = ola["connections"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last, json!({"kind":"same_title","count":13}));
    // The tiny map: the memory and its direct neighbors with plain labels.
    let map = &read["map"];
    let nodes = map["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["id"], "m-read");
    assert!(nodes.len() <= 13);
    assert!(nodes.iter().any(|n| n["label"] == "reading"));
    assert!(index.detail("m-missing").is_none());
    // The relation counts for both ends.
    let tax = index.detail("m-tax").unwrap();
    assert!(
        tax["connections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "relation" && c["target"]["id"] == "m-note")
    );
}

/// Hippocampus double that answers per user from [graph].
async fn hippocampus() -> Config {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        hippocampus_url: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let app = Router::new()
        .route(
            "/api/v1/graph",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                assert_eq!(q["limit"], "2000");
                assert_eq!(q["depth"], "1");
                Json(json!({"data": graph(&q["user_id"])}))
            }),
        )
        .route(
            "/api/v1/graph/nodes/{id}",
            get(|Path(id): Path<String>, Query(q): Query<HashMap<String, String>>| async move {
                let g = graph(&q["user_id"]);
                let node = g["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id.as_str()).cloned();
                match node {
                    Some(node) => Json(json!({"data":{"nodes":[node.clone()],"edges":[],"truncated":false,"node":node}})).into_response(),
                    None => (StatusCode::NOT_FOUND, Json(json!({"detail":"not found"}))).into_response(),
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    config
}
use axum::response::IntoResponse;

async fn call(state: &MemoryGraph, path: &str, user: &str) -> (StatusCode, Value) {
    let app = with_response_contract(router(state.clone()));
    let req = Request::get(path)
        .extension(axum::extract::ConnectInfo(
            "203.0.113.1:1234".parse::<std::net::SocketAddr>().unwrap(),
        ))
        .header("x-uuid", user);
    let response = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    (
        response.status(),
        serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn routes_scope_and_errors_without_chat_rows() {
    let config = hippocampus().await;
    let state = MemoryGraph::new(Hippocampus::new(&config).unwrap(), config.clone());
    let (status, body) = call(&state, "/memory-graph/overview?app_id=cortex", "alice").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "Memory overview loaded");
    assert_eq!(body["data"]["summary"]["memories"], 19);
    // Without chat rows every memory that is not a file is a saved note.
    assert_eq!(body["data"]["summary"]["notes"], 18);
    let (status, body) = call(&state, "/memory-graph/items?app_id=cortex&limit=5", "alice").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 19);
    // User B gets nothing from user A's data.
    let (_, body) = call(&state, "/memory-graph/items?app_id=cortex", "bob").await;
    assert_eq!(body["data"]["total"], 0);
    let (_, body) = call(&state, "/memory-graph/overview?app_id=cortex", "bob").await;
    assert_eq!(body["data"]["summary"]["memories"], 0);
    // The node route keeps its old fields and adds the detail.
    let (status, body) = call(&state, "/memory-graph/nodes/m-note?app_id=cortex", "alice").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["node"]["id"], "m-note");
    assert_eq!(body["data"]["item"]["id"], "m-note");
    assert!(body["data"]["connections"].is_array());
    let (status, body) = call(&state, "/memory-graph/nodes/m-note?app_id=cortex", "bob").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["data"]["error_number"], 1039);
    let (status, body) = call(&state, "/memory-graph/items?app_id=cortex&limit=0", "alice").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["error_number"], 1003);
    let (status, _) = call(
        &state,
        "/memory-graph/items?app_id=cortex&source=bad",
        "alice",
    )
    .await;
    assert!(status.is_client_error());
    let (status, body) = call(&state, "/memory-graph/overview?app_id=unknown", "alice").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["error_number"], 1009);
}

#[tokio::test]
#[ignore = "requires cortex_api_test; unique temporary schema, no cache"]
async fn sql_sources_usage_and_scope_on_the_test_database() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_overview_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let t = now();
    let chat = |user: &'static str, id: &'static str, title: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO chats(session_id,app_id,user_id,title,is_pinned,created_at,updated_at) VALUES($1::uuid,'cortex',$2,$3,false,$4,$4)")
                .bind(id).bind(user).bind(title).bind(t).execute(&pool).await.unwrap();
        }
    };
    let output = |session: &'static str, memory: Option<String>, at: DateTime<Utc>| {
        let pool = pool.clone();
        async move {
            let input = Uuid::now_v7();
            let output = Uuid::now_v7();
            sqlx::query("INSERT INTO chat_inputs(input_id,session_id,text,needs_web,web_mode,use_memory,thinking,created_at) VALUES($1,$2::uuid,'synthetic',false,'off',true,false,$3)")
                .bind(input).bind(session).bind(at).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO chat_outputs(output_id,input_id,session_id,status,text,memory_id,created_at) VALUES($1,$2,$3::uuid,'done','synthetic',$4,$5)")
                .bind(output).bind(input).bind(session).bind(memory).bind(at).execute(&pool).await.unwrap();
            (input, output)
        }
    };
    const BOB_CHAT: &str = "0190f000-0000-7000-8000-0000000000bb";
    chat("alice", CHAT_A, "Deploy problems").await;
    chat("alice", CHAT_B, "Reading").await;
    chat("alice", CHAT_C, "Taxes").await;
    chat("bob", BOB_CHAT, "Other user").await;
    for i in 0..13 {
        output(
            CHAT_A,
            Some(format!("m-ola-{i:02}")),
            t - Duration::minutes(i),
        )
        .await;
    }
    let (read_input, _) = output(CHAT_B, Some("m-read".into()), t - Duration::days(2)).await;
    output(CHAT_C, Some("m-tax".into()), t - Duration::days(10)).await;
    sqlx::query("INSERT INTO chat_input_files(input_id,file_id,memory_id,position) VALUES($1,'file-2','m-file-2',0)")
        .bind(read_input).execute(&pool).await.unwrap();
    for (session, at) in [
        (CHAT_C, t - Duration::days(1)),
        (CHAT_C, t - Duration::hours(3)),
        (BOB_CHAT, t),
    ] {
        let (_, out) = output(session, None, at).await;
        sqlx::query("INSERT INTO chat_output_memories(output_id,position,memory_id,title) VALUES($1,0,'m-read','Reading plan')")
            .bind(out).execute(&pool).await.unwrap();
    }
    let config = hippocampus().await;
    let state = MemoryGraph {
        records: Some(MemoryRecords { pool: pool.clone() }),
        cache: RedisCache::new(None, config.cache_operation_timeout),
        ..MemoryGraph::new(Hippocampus::new(&config).unwrap(), config.clone())
    };
    let started = std::time::Instant::now();
    let (status, body) = call(&state, "/memory-graph/overview?app_id=cortex", "alice").await;
    let overview_ms = started.elapsed().as_millis();
    assert_eq!(status, StatusCode::OK);
    let s = &body["data"]["summary"];
    assert_eq!(
        (
            s["memories"].as_u64(),
            s["chats"].as_u64(),
            s["files"].as_u64()
        ),
        (Some(19), Some(3), Some(2))
    );
    let (_, body) = call(&state, "/memory-graph/items?app_id=cortex", "alice").await;
    let items = body["data"]["items"].as_array().unwrap();
    let ola = items
        .iter()
        .find(|i| i["source"]["kind"] == "chat" && i["stack"]["count"] == 13);
    assert!(ola.is_some(), "13 ola memories merge into one item");
    let read = items.iter().find(|i| i["id"] == "m-read").unwrap();
    // Bob's answer that used the same memory id does not count for Alice.
    assert_eq!(read["used_in_answers"], 2);
    let (_, body) = call(&state, "/memory-graph/items?app_id=cortex", "bob").await;
    assert_eq!(body["data"]["total"], 0);
    let records = MemoryRecords { pool: pool.clone() };
    let bob = records
        .load(&cortex_api::domain::models::storage::AppUser {
            app_id: "cortex".into(),
            user_id: "bob".into(),
        })
        .await
        .unwrap();
    assert!(bob.iter().all(|r| r.session_id == BOB_CHAT));
    println!("overview with 19 memories: {overview_ms} ms");
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

/// Target of the issue: overview under 500 ms and items under 300 ms at 2000
/// memories. Synthetic data: 200 chats with 8 memories each, 400 notes.
#[test]
fn timing_at_2000_memories() {
    let t = now();
    let mut nodes = vec![];
    let mut rows = vec![];
    for i in 0..2000 {
        let id = format!("m-{i:04}");
        let at = t - Duration::minutes(i);
        nodes.push(memory(
            &id,
            "alice",
            &format!("Memory {}", i % 300),
            "episodic",
            at,
        ));
        if i < 1600 {
            let chat = format!("0190f000-0000-7000-8000-{:012}", i / 8);
            rows.push(record(
                "chat",
                &id,
                &chat,
                "Synthetic chat",
                &chat,
                at,
                None,
            ));
            if i % 5 == 0 {
                let out = format!("out-{i}");
                rows.push(record(
                    "used",
                    &id,
                    &chat,
                    "Synthetic chat",
                    &chat,
                    at,
                    Some(&out),
                ));
            }
        }
    }
    let graph = json!({"nodes": nodes, "edges": [], "truncated": false});
    let start = std::time::Instant::now();
    let index = Index::build(&graph, &rows);
    let overview = index.overview(t);
    let overview_ms = start.elapsed().as_millis();
    assert_eq!(overview["summary"]["memories"], 2000);
    let start = std::time::Instant::now();
    let page = index.items(&query(json!({})), t).unwrap();
    let items_ms = start.elapsed().as_millis();
    assert_eq!(page["total"], 2000);
    println!("2000 memories: index and overview {overview_ms} ms, items page {items_ms} ms");
    assert!(overview_ms < 500 && items_ms < 300);
}
