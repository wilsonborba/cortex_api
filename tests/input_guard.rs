use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};
use cortex_api::{
    core::{config::Config, process_numbers::INPUT_TOKEN_LIMIT},
    domain::{models::model_catalog::InputTooLarge, services::input_guard::check},
    presentation::{models, response::Envelope, with_response_contract},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;
#[derive(Deserialize)]
struct Input {
    text: String,
}
#[tokio::test]
async fn route_and_server_use_the_same_limit_and_rule() {
    let config = Config::default();
    let limit = config.max_input_tokens();
    let checked = config.clone();
    let app = with_response_contract(models::router(config.clone()).merge(Router::new().route(
        "/fixture-input",
        post(move |Json(input): Json<Input>| {
            let config = checked.clone();
            async move {
                check(&config, &input.text)?;
                Ok::<_, InputTooLarge>(Envelope::new("Input checked", json!({})))
            }
        }),
    )));
    let response = app
        .clone()
        .oneshot(Request::get("/models").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap();
    assert_eq!(body["data"]["max_input_tokens"], limit);
    assert_eq!(
        body["data"]["estimate_rule"],
        json!({"unit":"utf8_bytes","bytes_per_token":1})
    );
    assert_eq!(body["data"]["models"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["models"][0]["tier"], "t0");
    for (text, status) in [
        ("x".repeat(limit as usize), StatusCode::OK),
        ("x".repeat(limit as usize + 1), StatusCode::BAD_REQUEST),
        ("ก".repeat(limit as usize / 3 + 1), StatusCode::BAD_REQUEST),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/fixture-input")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"text":text}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::BAD_REQUEST {
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap())
                    .unwrap();
            assert_eq!(body["data"]["error_number"], INPUT_TOKEN_LIMIT.number);
        }
    }
}
#[test]
fn unicode_estimate_and_shared_reserves() {
    let config = Config::default();
    assert_eq!(config.estimate_tokens("Aก💡"), 8);
    assert_eq!(
        config.model_context_tokens,
        config.max_input_tokens()
            + config.estimate_tokens(&config.system_instruction)
            + config.chat_template_token_reserve
            + config.answer_token_reserve
            + config.thinking_notes_token_reserve
    );
    let smaller = Config {
        model_context_tokens: 4096,
        ..config.clone()
    };
    assert!(smaller.max_input_tokens() < config.max_input_tokens());
}
