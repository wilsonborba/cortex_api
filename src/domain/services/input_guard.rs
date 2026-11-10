use crate::{core::config::Config, domain::models::model_catalog::InputTooLarge};
/// Only the accepted user text enters this guard. Retrieved content is trimmed
/// by the assembler and cannot turn a valid input into an input-limit error.
pub fn check(config: &Config, text: &str) -> Result<(), InputTooLarge> {
    if config.estimate_tokens(text) > config.max_input_tokens() {
        Err(InputTooLarge)
    } else {
        Ok(())
    }
}
