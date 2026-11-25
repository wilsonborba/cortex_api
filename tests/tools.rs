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
fn action_digest_matches_shared_cli_golden_and_is_sensitive_to_uuid_tool_and_arguments() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/output_action_digest.json")).unwrap();
    let call_id = Uuid::parse_str(fixture["call_id"].as_str().unwrap()).unwrap();
    let parse_args = |raw: &str| serde_json::from_str::<serde_json::Value>(raw).unwrap();
    let name = ToolName::RunCommand;
    let first = parse_args(fixture["arguments_raw"][0].as_str().unwrap());
    let reordered = parse_args(fixture["arguments_raw"][1].as_str().unwrap());
    let expected = fixture["digest"].as_str().unwrap();

    assert_eq!(
        cortex_api::domain::models::tools::action_digest(call_id, name, &first),
        expected
    );
    assert_eq!(
        cortex_api::domain::models::tools::action_digest(call_id, name, &reordered),
        expected,
        "reordered nested object keys must preserve the digest"
    );
    assert_eq!(
        cortex_api::domain::models::tools::action_digest(
            Uuid::parse_str(fixture["changed_call_id"]["value"].as_str().unwrap()).unwrap(),
            name,
            &first,
        ),
        fixture["changed_call_id"]["digest"].as_str().unwrap()
    );
    assert_eq!(
        cortex_api::domain::models::tools::action_digest(call_id, ToolName::ViewFile, &first),
        fixture["changed_tool_name"]["digest"].as_str().unwrap()
    );
    assert_eq!(
        cortex_api::domain::models::tools::action_digest(
            call_id,
            name,
            &parse_args(fixture["changed_arguments"]["raw"].as_str().unwrap()),
        ),
        fixture["changed_arguments"]["digest"].as_str().unwrap()
    );
    assert_eq!(
        cortex_api::domain::models::tools::action_digest(
            call_id,
            name,
            &parse_args(fixture["changed_array_order"]["raw"].as_str().unwrap()),
        ),
        fixture["changed_array_order"]["digest"].as_str().unwrap()
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
