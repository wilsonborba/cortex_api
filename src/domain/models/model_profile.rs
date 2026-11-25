use crate::core::config::Config;
use serde::{Deserialize, Serialize};

/// Stable, allowlisted model IDs accepted by the public chat API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelProfile {
    #[serde(rename = "cortex-t0")]
    CortexT0,
    #[serde(rename = "bonsai25-uncensored")]
    Bonsai25Uncensored,
    #[serde(rename = "bonsai25-official")]
    Bonsai25Official,
}

impl ModelProfile {
    pub const fn id(self) -> &'static str {
        match self {
            Self::CortexT0 => "cortex-t0",
            Self::Bonsai25Uncensored => "bonsai25-uncensored",
            Self::Bonsai25Official => "bonsai25-official",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "cortex-t0" => Some(Self::CortexT0),
            "bonsai25-uncensored" => Some(Self::Bonsai25Uncensored),
            "bonsai25-official" => Some(Self::Bonsai25Official),
            _ => None,
        }
    }

    pub const fn tier(self) -> &'static str {
        match self {
            Self::CortexT0 => "t0",
            Self::Bonsai25Uncensored | Self::Bonsai25Official => "t1",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CortexT0 => "Cortex T0",
            Self::Bonsai25Uncensored => "Bonsai 2.5 27B Uncensored",
            Self::Bonsai25Official => "Bonsai 2.5 27B Official",
        }
    }

    pub const fn capabilities(self) -> &'static [&'static str] {
        match self {
            Self::CortexT0 => &["text", "vision", "prompted-json", "tool-calls"],
            Self::Bonsai25Uncensored | Self::Bonsai25Official => {
                &["text", "prompted-json", "tool-calls"]
            }
        }
    }

    pub const fn uses_prism(self) -> bool {
        !matches!(self, Self::CortexT0)
    }

    /// Distinct runtime aliases let readiness checks reject a swapped model.
    pub const fn runtime_alias(self) -> Option<&'static str> {
        match self {
            Self::CortexT0 => None,
            Self::Bonsai25Uncensored => Some("bonsai25-uncensored"),
            Self::Bonsai25Official => Some("bonsai25-official"),
        }
    }

    pub const fn artifact_filename(self) -> Option<&'static str> {
        match self {
            Self::CortexT0 => None,
            Self::Bonsai25Uncensored => Some("bonsai25-atomicchat-uncensored-PTQ1_0.gguf"),
            Self::Bonsai25Official => Some("bonsai25-official-PTQ1_0-mtp.gguf"),
        }
    }

    pub const fn artifact_sha256(self) -> Option<&'static str> {
        match self {
            Self::CortexT0 => None,
            Self::Bonsai25Uncensored => {
                Some("f6b5f856c3981ff5ba1db1ce13cdf1ca85f2f701d26ce287e03983b659b681b4")
            }
            Self::Bonsai25Official => {
                Some("e0fc5f3d47b9c926d52677d303b19df356a2376614b3e1cb9973865fc04ca431")
            }
        }
    }

    pub const fn uses_mtp(self) -> bool {
        matches!(self, Self::Bonsai25Official)
    }

    pub const fn context_tokens(self, config: &Config) -> u64 {
        match self {
            Self::CortexT0 => config.model_context_tokens,
            Self::Bonsai25Uncensored | Self::Bonsai25Official => 8192,
        }
    }

    pub const fn max_output_tokens(self, config: &Config) -> u64 {
        match self {
            Self::CortexT0 => config.answer_token_reserve,
            Self::Bonsai25Uncensored | Self::Bonsai25Official => 2048,
        }
    }

    pub fn max_input_tokens(self, config: &Config) -> u64 {
        self.context_tokens(config)
            .saturating_sub(config.system_budget_tokens())
            .saturating_sub(self.max_output_tokens(config))
            .saturating_sub(config.thinking_notes_token_reserve)
    }

    pub const fn legacy_default() -> Self {
        Self::CortexT0
    }
}

impl Default for ModelProfile {
    /// Newly created chats use the owner-selected default T1 profile.
    fn default() -> Self {
        Self::Bonsai25Uncensored
    }
}

impl std::fmt::Display for ModelProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::ModelProfile;

    #[test]
    fn public_ids_are_allowlisted_and_stable() {
        assert_eq!(
            ModelProfile::from_id("cortex-t0"),
            Some(ModelProfile::CortexT0)
        );
        assert_eq!(
            ModelProfile::from_id("bonsai25-uncensored"),
            Some(ModelProfile::Bonsai25Uncensored)
        );
        assert_eq!(
            ModelProfile::from_id("bonsai25-official"),
            Some(ModelProfile::Bonsai25Official)
        );
        assert_eq!(ModelProfile::from_id("/arbitrary/model.gguf"), None);
    }

    #[test]
    fn new_and_legacy_defaults_are_distinct() {
        assert_eq!(ModelProfile::default(), ModelProfile::Bonsai25Uncensored);
        assert_eq!(ModelProfile::legacy_default(), ModelProfile::CortexT0);
    }
}
