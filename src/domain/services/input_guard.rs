use crate::{core::config::Config, domain::models::model_catalog::InputTooLarge};
/// Only the accepted user text enters this guard. Retrieved content is trimmed
/// by the assembler and cannot turn a valid input into an input-limit error.
pub fn check(config: &Config, text: &str) -> Result<(), InputTooLarge> {
    check_max(config, text, config.max_input_tokens())
}

pub fn check_profile(
    config: &Config,
    text: &str,
    context_tokens: u64,
    output_tokens: u64,
) -> Result<(), InputTooLarge> {
    let max = context_tokens
        .saturating_sub(config.system_budget_tokens())
        .saturating_sub(output_tokens)
        .saturating_sub(config.thinking_notes_token_reserve);
    check_max(config, text, max)
}

fn check_max(config: &Config, text: &str, max_tokens: u64) -> Result<(), InputTooLarge> {
    if config.estimate_input_tokens(text) > max_tokens {
        Err(InputTooLarge)
    } else {
        Ok(())
    }
}
