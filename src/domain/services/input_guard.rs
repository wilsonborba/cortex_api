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

#[cfg(test)]
mod tests {
    use super::check_profile;
    use crate::{core::config::Config, domain::models::model_profile::ModelProfile};

    #[test]
    fn both_t1_profiles_accept_their_estimated_input_boundary_and_reject_one_step_over() {
        let config = Config::default();
        for profile in [
            ModelProfile::Bonsai25Uncensored,
            ModelProfile::Bonsai25Official,
        ] {
            let context = profile.context_tokens(&config);
            let output = profile.max_output_tokens(&config);
            let input_limit = profile.max_input_tokens(&config);
            assert_eq!(context, 8192, "{} context", profile.id());
            assert_eq!(
                input_limit
                    + config.system_budget_tokens()
                    + output
                    + config.thinking_notes_token_reserve,
                context,
                "{} accounting",
                profile.id()
            );

            // Find the largest ASCII input accepted under the configured estimator,
            // then prove the next byte crosses the guard. This tests accounting,
            // not model retrieval quality at long context.
            let mut low = 0usize;
            let mut high = input_limit as usize + 1;
            while low + 1 < high {
                let mid = low + (high - low) / 2;
                if config.estimate_input_tokens(&"x".repeat(mid)) <= input_limit {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            let at_boundary = "x".repeat(low);
            assert!(
                check_profile(&config, &at_boundary, context, output).is_ok(),
                "{} rejected estimated boundary input ({})",
                profile.id(),
                config.estimate_input_tokens(&at_boundary)
            );
            let over_boundary = format!("{at_boundary}x");
            assert!(
                check_profile(&config, &over_boundary, context, output).is_err(),
                "{} accepted input beyond its limit",
                profile.id()
            );

            // UTF-8 is counted by bytes, so a multibyte string must obey the same
            // configured estimator rather than a character-count shortcut.
            let thai = "ก".repeat(low / "ก".len());
            assert!(
                check_profile(&config, &thai, context, output).is_ok(),
                "{} rejected multibyte input within its estimated limit",
                profile.id()
            );
        }
    }
}
