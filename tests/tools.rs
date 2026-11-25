use cortex_api::domain::models::tools::{
    PendingTool, ToolControl, ToolName, ToolRequest, ToolResult,
};
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

#[test]
fn provider_requests_are_allowlisted_strict_and_bounded() {
    let request = ToolRequest::from_provider(
        "view_file",
        json!({"path":"src/main.rs","start_line":2,"end_line":4}),
    )
    .unwrap();
    assert_eq!(request.name, ToolName::ViewFile);
    assert_eq!(request.arguments["path"], "src/main.rs");
    assert!(ToolRequest::from_provider("shell", json!({"command":"whoami"})).is_err());
    assert!(
        ToolRequest::from_provider(
            "view_file",
            json!({"path":"src/main.rs","read_outside_workspace":true}),
        )
        .is_err()
    );
    assert!(
        ToolRequest::from_provider(
            "view_file",
            json!({"path":"src/main.rs","start_line":1,"end_line":502}),
        )
        .is_err()
    );
    for arguments in [
        json!({"path":"src/main.rs","start_line":0}),
        json!({"path":"src/main.rs","end_line":0}),
        json!({"path":"src/main.rs","start_line":0,"end_line":0}),
        json!({"path":"src/main.rs","start_line":1,"end_line":501}),
    ] {
        assert!(ToolRequest::from_provider("view_file", arguments).is_err());
    }
    assert!(
        ToolRequest::from_provider(
            "view_file",
            json!({"path":"src/main.rs","start_line":1,"end_line":500}),
        )
        .is_ok()
    );
    assert!(ToolRequest::from_provider("grep_search", json!({"query":"x","paths":[]})).is_err());
}

#[test]
fn action_digest_covers_call_name_and_canonical_arguments() {
    let digest = cortex_api::domain::models::tools::action_digest(
        Uuid::parse_str("30000000-0000-4000-8000-000000000003").unwrap(),
        ToolName::RunCommand,
        &json!({"cwd":".","argv":["echo","ok"]}),
    );
    assert_eq!(
        digest,
        "52f61567c4934e6c0ea1da29e80bb22fe2f08756ad1fc28b81ddace856de9c33"
    );
}

#[tokio::test]
async fn tool_result_requires_matching_pending_call_and_first_result_wins() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let control = ToolControl::new(tx);
    let call_id = Uuid::now_v7();
    assert!(control.set_pending(PendingTool {
        call_id,
        name: ToolName::RunCommand,
        arguments: json!({"cwd":".","argv":["echo","ok"]}),
    }));
    assert!(
        control
            .submit(ToolResult {
                call_id,
                name: ToolName::ViewFile,
                output: "wrong tool".into(),
            })
            .is_err()
    );
    control
        .submit(ToolResult {
            call_id,
            name: ToolName::RunCommand,
            output: "finished".into(),
        })
        .unwrap();
    assert!(
        control
            .submit(ToolResult {
                call_id,
                name: ToolName::RunCommand,
                output: "replay".into(),
            })
            .is_err()
    );
    let received = rx.recv().await.unwrap();
    assert_eq!(received.output, "finished");
}
