use crate::core::config::{Config, TokenEstimateRule};
use serde::Serialize;
#[derive(Serialize)]
pub struct ActiveModel {
    pub id: &'static str,
    pub tier: &'static str,
    pub name: String,
}
#[derive(Serialize)]
pub struct ModelsData {
    pub models: Vec<ActiveModel>,
    pub max_input_tokens: u64,
    pub estimate_rule: TokenEstimateRule,
}
impl From<&Config> for ModelsData {
    fn from(config: &Config) -> Self {
        Self {
            models: vec![ActiveModel {
                id: "cortex-t0",
                tier: "t0",
                name: config.text_model.clone(),
            }],
            max_input_tokens: config.max_input_tokens(),
            estimate_rule: config.input_estimate_rule(),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct InputTooLarge;
