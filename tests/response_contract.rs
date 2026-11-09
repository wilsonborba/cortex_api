use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::{get, post},
};
use cortex_api::{
    core::process_numbers::*,
    presentation::{UserId, response::Envelope, with_response_contract},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Deserialize)]
struct Input {
    text: String,
}
fn app() -> Router {
    with_response_contract(
        Router::new()
            .route(
                "/success",
                get(|| async { Envelope::new("Chat created", json!({"session_id":"a"})) }),
            )
            .route(
                "/json",
                post(|Json(input): Json<Input>| async move {
                    Envelope::new("Input accepted", json!({"text":input.text}))
                }),
            )
            .route(
                "/user",
                get(|UserId(user): UserId| async move {
                    Envelope::new("User found", json!({"user_id":user}))
                }),
            ),
    )
}
async fn check(method: &str, path: &str, body: &str, status: StatusCode, number: u16) {
    let response = app()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), status);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 2);
    assert_eq!(value["data"]["error_number"], number);
    assert!(value["message"].is_string());
}
#[tokio::test]
async fn framework_errors() {
    check(
        "GET",
        "/missing",
        "",
        StatusCode::NOT_FOUND,
        NOT_FOUND.number,
    )
    .await;
    check(
        "POST",
        "/success",
        "",
        StatusCode::METHOD_NOT_ALLOWED,
        WRONG_METHOD.number,
    )
    .await;
    check(
        "POST",
        "/json",
        "{}",
        StatusCode::UNPROCESSABLE_ENTITY,
        INVALID_DATA.number,
    )
    .await;
    check(
        "POST",
        "/json",
        "{",
        StatusCode::BAD_REQUEST,
        BAD_JSON.number,
    )
    .await;
    check(
        "GET",
        "/user",
        "",
        StatusCode::BAD_REQUEST,
        MISSING_USER.number,
    )
    .await;
}
#[tokio::test]
async fn success_shape() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/success")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(
        value,
        json!({"message":"Chat created","data":{"session_id":"a"}})
    );
}
#[test]
fn notice_shape() {
    // This test number is not part of the public registry.
    let process = ProcessNumber {
        number: 9000,
        message: "Test notice",
    };
    let value = serde_json::to_value(Envelope::notice(process, json!({"stage":"memory"}))).unwrap();
    assert_eq!(
        value,
        json!({"message":"Test notice","data":{"notice_number":9000,"stage":"memory"}})
    );
}
#[test]
fn registry_is_unique_and_exportable() {
    let numbers: std::collections::HashSet<_> = REGISTRY.iter().map(|p| p.number).collect();
    assert_eq!(numbers.len(), REGISTRY.len());
    println!("{}", serde_json::to_string_pretty(REGISTRY).unwrap());
}
