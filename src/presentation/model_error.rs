use crate::{
    core::process_numbers::{self, ProcessNumber},
    domain::models::{driver::DriverError, usage::QueueError},
};
/// Cancellation is control flow. The output stream reports its final status.
pub fn number(error: DriverError) -> Option<ProcessNumber> {
    match error {
        DriverError::Queue(QueueError::HeavyLoad) => Some(process_numbers::HEAVY_LOAD),
        DriverError::Queue(QueueError::ServerFault) => Some(process_numbers::QUEUE_FAULT),
        DriverError::InvalidInput => Some(process_numbers::INVALID_DATA),
        DriverError::Ollama => Some(process_numbers::OLLAMA_FAILED),
        DriverError::Cancelled => None,
    }
}
