use crate::{
    core::{config::Config, process_numbers},
    domain::models::identity::{Caller, IdentityError},
};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::Response,
};
use std::net::SocketAddr;
/// Headers carry the trusted gateway or LAN-backend identity. The socket peer
/// is used only when no user is present. Forwarded headers do not set the peer.
pub struct CallerContext {
    user: Option<String>,
    admin: bool,
    peer: Option<std::net::IpAddr>,
}
impl CallerContext {
    pub fn for_app(&self, app_id: &str, require_user: bool) -> Result<Caller, IdentityError> {
        crate::domain::services::identity::attribute(
            &Config::default(),
            app_id,
            self.user.as_deref(),
            self.admin,
            self.peer,
            require_user,
        )
    }
}
impl<S: Send + Sync> FromRequestParts<S> for CallerContext {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Response> {
        let user = match parts.headers.get("x-uuid") {
            None => None,
            Some(value) => Some(
                value
                    .to_str()
                    .ok()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        super::response::error(
                            StatusCode::BAD_REQUEST,
                            process_numbers::MISSING_USER,
                        )
                    })?
                    .to_owned(),
            ),
        };
        Ok(Self {
            user,
            admin: parts
                .headers
                .get("x-access-level")
                .and_then(|s| s.to_str().ok())
                == Some("admin"),
            peer: parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|v| v.0.ip()),
        })
    }
}

impl axum::response::IntoResponse for IdentityError {
    fn into_response(self) -> Response {
        match self {
            Self::UnknownApp => {
                super::response::error(StatusCode::BAD_REQUEST, process_numbers::UNKNOWN_APP)
            }
            Self::MissingUser => {
                super::response::error(StatusCode::BAD_REQUEST, process_numbers::MISSING_USER)
            }
        }
    }
}
