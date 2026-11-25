use crate::{
    core::config::{Config, TokenEstimateRule},
    domain::models::model_profile::ModelProfile,
};
use serde::Serialize;
#[derive(Serialize)]
pub struct ActiveModel {
    pub id: &'static str,
    pub tier: &'static str,
    pub name: String,
    pub context_tokens: u64,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub capabilities: &'static [&'static str],
}
#[derive(Serialize)]
pub struct ModelsData {
    pub models: Vec<ActiveModel>,
    pub default_model_id: &'static str,
    pub max_input_tokens: u64,
    pub estimate_rule: TokenEstimateRule,
}
impl From<&Config> for ModelsData {
    fn from(config: &Config) -> Self {
        Self {
            models: [
                ModelProfile::CortexT0,
                ModelProfile::Bonsai25Uncensored,
                ModelProfile::Bonsai25Official,
            ]
            .into_iter()
            .filter(|profile| config.bonsai_enabled || !profile.uses_prism())
            .map(|profile| ActiveModel {
                id: profile.id(),
                tier: profile.tier(),
                name: match profile {
                    ModelProfile::CortexT0 => config.text_model.clone(),
                    _ => profile.display_name().into(),
                },
                context_tokens: profile.context_tokens(config),
                max_input_tokens: profile.max_input_tokens(config),
                max_output_tokens: profile.max_output_tokens(config),
                capabilities: profile.capabilities(),
            })
            .collect(),
            default_model_id: if config.bonsai_enabled {
                ModelProfile::default().id()
            } else {
                ModelProfile::legacy_default().id()
            },
            max_input_tokens: config.max_input_tokens(),
            estimate_rule: config.input_estimate_rule(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ModelsData;
    use crate::{core::config::Config, domain::models::model_profile::ModelProfile};

    #[test]
    fn tier_one_is_hidden_until_enabled_then_uncensored_is_default() {
        let mut config = Config::default();
        let disabled = ModelsData::from(&config);
        assert_eq!(disabled.models.len(), 1);
        assert_eq!(disabled.default_model_id, "cortex-t0");
        config.bonsai_enabled = true;
        let enabled = ModelsData::from(&config);
        assert_eq!(enabled.models.len(), 3);
        assert_eq!(enabled.default_model_id, "bonsai25-uncensored");
        assert_eq!(ModelProfile::Bonsai25Official.context_tokens(&config), 8192);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InputTooLarge;
