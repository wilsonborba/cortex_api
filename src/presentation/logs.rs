use crate::{core::logs::LogHub, domain::services::identity::lan};
use axum::{
    Router,
    extract::{ConnectInfo, State},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::get,
};
use std::{convert::Infallible, net::SocketAddr};
pub fn router(hub: LogHub) -> Router {
    Router::new()
        .route("/logs/stream", get(stream))
        .with_state(hub)
}
async fn stream(State(hub): State<LogHub>, request: axum::extract::Request) -> Response {
    if !request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|peer| lan(peer.0.ip()))
    {
        return crate::presentation::response::error(
            axum::http::StatusCode::FORBIDDEN,
            crate::core::process_numbers::LOG_LAN_ONLY,
        );
    }
    let events = futures_util::stream::unfold(hub.subscribe(), |mut receiver| async move {
        let event = match receiver.recv().await {
            Ok(line) => Event::default().event("log").data(line),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => Event::default()
                .event("gap")
                .data(format!("{count} log events skipped")),
            Err(_) => return None,
        };
        Some((Ok::<_, Infallible>(event), receiver))
    });
    let mut response = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}
