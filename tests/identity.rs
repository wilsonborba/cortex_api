use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
    routing::post,
};
use cortex_api::{
    core::{
        config::Config,
        process_numbers::{MISSING_USER, UNKNOWN_APP},
    },
    domain::{models::identity::IdentityError, services::identity::attribute},
    presentation::{identity::CallerContext, response::Envelope, with_response_contract},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;
use tower::ServiceExt;
#[derive(Deserialize)]
struct Input {
    app_id: String,
}
fn app() -> Router {
    with_response_contract(Router::new().route("/fixture",post(|context:CallerContext,Json(input):Json<Input>|async move{let caller=context.for_app(&input.app_id,false)?;Ok::<_,IdentityError>(Envelope::new("Attributed",json!({"scope":caller.memory_scope(),"balancer":caller.balancer_identity(),"admin":caller.is_admin,"saved_user":caller.storage_scope().user_id})))})).route("/user",post(|context:CallerContext,Json(input):Json<Input>|async move{let caller=context.for_app(&input.app_id,true)?;Ok::<_,IdentityError>(Envelope::new("Attributed",json!({"user":caller.user_id})))})))
}
async fn call(
    path: &str,
    peer: SocketAddr,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::post(path)
        .extension(ConnectInfo(peer))
        .header("content-type", "application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let response = app()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    (
        response.status(),
        serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap()).unwrap(),
    )
}
#[tokio::test]
async fn header_user_catalog_app_lan_a2a_and_admin() {
    let lan = "127.0.0.1:1234".parse().unwrap();
    let public = "203.0.113.1:1234".parse().unwrap();
    let (status, value) = call(
        "/fixture",
        public,
        &[("x-uuid", "real-user"), ("x-access-level", "admin")],
        json!({"app_id":"cortex","user_id":"forged-body-user","tenant_id":"ignored"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value["data"]["scope"],
        json!({"workspace_id":"cortex","app_id":"cortex","user_id":"real-user"})
    );
    assert_eq!(value["data"]["admin"], true);
    assert_eq!(value["data"]["balancer"], "real-user");
    let (status, value) = call("/fixture", lan, &[], json!({"app_id":"certifications"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["data"]["scope"]["user_id"], Value::Null);
    assert_eq!(value["data"]["balancer"], "app:certifications");
    assert_eq!(value["data"]["saved_user"], "");
    for (path, peer, headers, number) in [
        ("/user", lan, vec![], MISSING_USER.number),
        ("/fixture", public, vec![], MISSING_USER.number),
        (
            "/fixture",
            public,
            vec![("x-forwarded-for", "127.0.0.1")],
            MISSING_USER.number,
        ),
    ] {
        let (status, body) = call(path, peer, &headers, json!({"app_id":"cortex"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["data"]["error_number"], number);
    }
    let (status, body) = call(
        "/fixture",
        lan,
        &[("x-uuid", "user")],
        json!({"app_id":"unknown"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["data"]["error_number"], UNKNOWN_APP.number);
}
#[test]
fn every_catalog_app_and_missing_peer() {
    let c = Config::default();
    assert_eq!(c.allowed_apps.len(), 9);
    for app in &c.allowed_apps {
        assert!(attribute(&c, app, Some("user"), false, None, true).is_ok());
    }
    assert_eq!(
        attribute(&c, "cortex", None, false, None, false),
        Err(IdentityError::MissingUser)
    );
}
