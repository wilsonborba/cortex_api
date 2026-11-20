use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use cortex_api::domain::services::remote_cli::{RemoteCliService, RemoteEventKind};
use cortex_api::presentation::{remote_cli, with_response_contract};

async fn json_req(
    app: &axum::Router,
    method: &str,
    uri: &str,
    user: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-uuid", user)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn remote_cli_isolation_cursor_replay_first_approval_wins_and_cancel() {
    let svc = RemoteCliService::new(5_000, 1_000);
    let app = with_response_contract(remote_cli::routes(svc.clone()));

    // Register session as user A
    let (st, reg) = json_req(
        &app,
        "POST",
        "/api/v1/remote/sessions",
        "user-a",
        serde_json::json!({ "device_name": "laptop", "platform": "linux" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = reg["session_id"].as_str().unwrap().to_string();

    // User B cannot list or control user A's session
    let (st_b_list, list_b) = json_req(&app, "GET", "/api/v1/remote/sessions", "user-b", serde_json::Value::Null).await;
    assert_eq!(st_b_list, StatusCode::OK);
    assert!(list_b["sessions"].as_array().unwrap().is_empty());

    let (st_b_prompt, _) = json_req(
        &app,
        "POST",
        &format!("/api/v1/remote/sessions/{sid}/prompts"),
        "user-b",
        serde_json::json!({ "prompt": "hijack" }),
    )
    .await;
    assert_eq!(st_b_prompt, StatusCode::FORBIDDEN);

    // Send prompt + approval request
    let (st_p, p_ev) = json_req(
        &app,
        "POST",
        &format!("/api/v1/remote/sessions/{sid}/prompts"),
        "user-a",
        serde_json::json!({ "prompt": "run tests", "file_ids": ["fsm-1"] }),
    )
    .await;
    assert_eq!(st_p, StatusCode::ACCEPTED);
    let first_cmd_id = p_ev["id"].as_u64().unwrap();

    // CLI posts progress and approval request
    let (st_ev, _) = json_req(
        &app,
        "POST",
        &format!("/api/v1/remote/sessions/{sid}/events"),
        "user-a",
        serde_json::json!({
            "kind": "approval_request",
            "payload": { "approval_id": "apr-1", "action": "git push" },
            "ack_command_id": first_cmd_id
        }),
    )
    .await;
    assert_eq!(st_ev, StatusCode::CREATED);

    // Approval answered twice: first succeeds, second is rejected (409 Conflict)
    let (st_a1, _) = json_req(
        &app,
        "POST",
        &format!("/api/v1/remote/sessions/{sid}/approvals/apr-1"),
        "user-a",
        serde_json::json!({ "approved": true }),
    )
    .await;
    assert_eq!(st_a1, StatusCode::OK);

    let (st_a2, _) = json_req(
        &app,
        "POST",
        &format!("/api/v1/remote/sessions/{sid}/approvals/apr-1"),
        "user-a",
        serde_json::json!({ "approved": false }),
    )
    .await;
    assert_eq!(st_a2, StatusCode::CONFLICT);

    // Cancel delivers Cancel command to CLI stream
    let (st_c, _) = json_req(
        &app,
        "POST",
        &format!("/api/v1/remote/sessions/{sid}/cancel"),
        "user-a",
        serde_json::json!({ "turn_id": "t-1" }),
    )
    .await;
    assert_eq!(st_c, StatusCode::ACCEPTED);

    // Cursor replay returns only events after cursor without loss or duplication
    let (cmds_after_1, _) = svc.cli_commands_since(&sid, "user-a", first_cmd_id).await.unwrap();
    assert_eq!(cmds_after_1.len(), 2);
    assert_eq!(cmds_after_1[0].kind, RemoteEventKind::ApprovalAnswer);
    assert_eq!(cmds_after_1[1].kind, RemoteEventKind::Cancel);

    // Pending command expiry & offline detection
    svc.set_cli_connected(&sid, "user-a", false, 100).await.unwrap();
    assert!(!svc.list_sessions("user-a", 10_000).await[0].online);
    assert!(svc.expire_pending_commands(u64::MAX).await >= 1);
}
