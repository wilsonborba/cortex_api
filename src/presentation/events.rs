use crate::{domain::models::events::OutputEvent, presentation::response::Envelope};
use axum::response::sse::{Event, Sse};
use futures_util::{Stream, stream};
use std::convert::Infallible;
use tokio::sync::mpsc;
pub fn sse(
    receiver: mpsc::UnboundedReceiver<OutputEvent>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(stream::unfold(receiver, |mut receiver| async move {
        let output = receiver.recv().await?;
        let event = Event::default()
            .id(output.id.to_string())
            .event(output.event.name())
            .json_data(Envelope::new(output.event.message(), output.event.data()))
            .expect("event data serializes");
        Some((Ok(event), receiver))
    }))
}
