use crate::{
    core::process_numbers,
    domain::{models::personas::*, services::personas::Personas},
    presentation::{
        identity::CallerContext,
        response::{self, Envelope},
    },
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
pub fn router(state: Personas) -> Router {
    Router::new()
        .route("/personas", get(list).post(create))
        .route("/personas/schemas/{type}", get(schema))
        .route(
            "/personas/{persona_id}",
            get(one).patch(change).delete(remove),
        )
        .with_state(state)
}
fn failure(error: PersonaError) -> Response {
    match error {
        PersonaError::Invalid(errors) => {
            let process = process_numbers::PERSONA_INVALID;
            (
                StatusCode::BAD_REQUEST,
                Envelope::new(
                    process.message,
                    json!({"error_number": process.number, "errors": errors}),
                ),
            )
                .into_response()
        }
        PersonaError::Process(process) => {
            let status = match process.number {
                n if n == process_numbers::PERSONA_NOT_FOUND.number => StatusCode::NOT_FOUND,
                n if n == process_numbers::PERSONA_UNAVAILABLE.number
                    || n == process_numbers::SECURITY_UNAVAILABLE.number =>
                {
                    StatusCode::SERVICE_UNAVAILABLE
                }
                _ => StatusCode::BAD_REQUEST,
            };
            response::error(status, process)
        }
    }
}
/// Personas need the user: scope is workspace cortex, app_id and x-uuid.
fn caller(
    context: &CallerContext,
    app_id: &str,
) -> Result<crate::domain::models::identity::Caller, crate::domain::models::identity::IdentityError>
{
    context.for_app(app_id, true)
}
async fn list(
    State(state): State<Personas>,
    context: CallerContext,
    Query(query): Query<PersonaQuery>,
) -> Result<Envelope<Vec<Persona>>, Response> {
    let caller = caller(&context, &query.app_id).map_err(IntoResponse::into_response)?;
    state
        .list(&caller, &query)
        .await
        .map(|v| Envelope::new("Personas loaded", v))
        .map_err(failure)
}
async fn create(
    State(state): State<Personas>,
    context: CallerContext,
    Query(query): Query<AppQuery>,
    Json(body): Json<NewPersona>,
) -> Result<Response, Response> {
    let caller = caller(&context, &query.app_id).map_err(IntoResponse::into_response)?;
    let persona = state.create(&caller, body).await.map_err(failure)?;
    Ok((
        StatusCode::CREATED,
        Envelope::new("Persona created", persona),
    )
        .into_response())
}
async fn one(
    State(state): State<Personas>,
    context: CallerContext,
    Path(id): Path<String>,
    Query(query): Query<AppQuery>,
) -> Result<Envelope<Persona>, Response> {
    let caller = caller(&context, &query.app_id).map_err(IntoResponse::into_response)?;
    state
        .get(&caller, &id)
        .await
        .map(|v| Envelope::new("Persona loaded", v))
        .map_err(failure)
}
async fn change(
    State(state): State<Personas>,
    context: CallerContext,
    Path(id): Path<String>,
    Query(query): Query<AppQuery>,
    Json(body): Json<PersonaChange>,
) -> Result<Envelope<Persona>, Response> {
    let caller = caller(&context, &query.app_id).map_err(IntoResponse::into_response)?;
    state
        .update(&caller, &id, body)
        .await
        .map(|v| Envelope::new("Persona changed", v))
        .map_err(failure)
}
async fn remove(
    State(state): State<Personas>,
    context: CallerContext,
    Path(id): Path<String>,
    Query(query): Query<AppQuery>,
) -> Result<Envelope<Value>, Response> {
    let caller = caller(&context, &query.app_id).map_err(IntoResponse::into_response)?;
    state
        .delete(&caller, &id)
        .await
        .map(|()| Envelope::new("Persona deleted", json!({"persona_id": id})))
        .map_err(failure)
}
async fn schema(
    State(state): State<Personas>,
    context: CallerContext,
    Path(kind): Path<PersonaType>,
    Query(query): Query<AppQuery>,
) -> Result<Envelope<Value>, Response> {
    let caller = caller(&context, &query.app_id).map_err(IntoResponse::into_response)?;
    state
        .schema(&caller, kind)
        .await
        .map(|v| Envelope::new("Persona schema loaded", v))
        .map_err(failure)
}
