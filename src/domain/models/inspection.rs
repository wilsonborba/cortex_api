//! Recorded model output data. These values do not explain model reasoning.
use serde::{Deserialize, Serialize};

pub const ALTERNATIVES: usize = 5;
const MAX_TOKEN_BYTES: usize = 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alternative {
    pub bytes: Vec<u8>,
    pub logprob: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InspectionToken {
    pub start_byte: usize,
    pub end_byte: usize,
    pub bytes: Vec<u8>,
    pub logprob: f64,
    pub alternatives: Vec<Alternative>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerInspection {
    pub model: String,
    pub reason: Option<String>,
    pub tokens: Vec<InspectionToken>,
}
impl AnswerInspection {
    pub fn matches(&self, text: &str) -> bool {
        if self.reason.is_some() {
            return self.tokens.is_empty();
        }
        let mut offset = 0;
        for token in &self.tokens {
            if token.start_byte != offset
                || token.end_byte != offset + token.bytes.len()
                || token.end_byte > i32::MAX as usize
                || !token.logprob.is_finite()
                || token.logprob > 0.0
                || token.bytes.len() > MAX_TOKEN_BYTES
                || token.alternatives.len() > ALTERNATIVES
                || token.alternatives.iter().any(|a| {
                    !a.logprob.is_finite() || a.logprob > 0.0 || a.bytes.len() > MAX_TOKEN_BYTES
                })
            {
                return false;
            }
            offset = token.end_byte;
        }
        !self.tokens.is_empty()
            && self
                .tokens
                .iter()
                .flat_map(|t| t.bytes.iter().copied())
                .eq(text.bytes())
    }
    pub fn unavailable(model: &str, reason: &str) -> Self {
        Self {
            model: model.into(),
            reason: Some(reason.into()),
            tokens: vec![],
        }
    }
}
#[derive(Default)]
pub struct InspectionCapture {
    tokens: Vec<InspectionToken>,
    invalid: bool,
}
fn bytes(value: &serde_json::Value) -> Option<Vec<u8>> {
    let result = if let Some(raw) = value.get("bytes").filter(|v| !v.is_null()) {
        raw.as_array()?
            .iter()
            .map(|v| u8::try_from(v.as_u64()?).ok())
            .collect::<Option<Vec<_>>>()?
    } else {
        value.get("token")?.as_str()?.as_bytes().to_vec()
    };
    (result.len() <= MAX_TOKEN_BYTES).then_some(result)
}
fn probability(value: &serde_json::Value) -> Option<f64> {
    value
        .get("logprob")?
        .as_f64()
        .filter(|p| p.is_finite() && *p <= 0.0)
}
impl InspectionCapture {
    pub fn push(&mut self, value: Option<&serde_json::Value>, limit: usize) {
        if self.invalid {
            return;
        }
        let Some(value) = value.filter(|v| !v.is_null()) else {
            return;
        };
        let parsed = (|| -> Option<Vec<InspectionToken>> {
            let values = value.as_array()?;
            if values.len() > limit.saturating_sub(self.tokens.len()) {
                return None;
            }
            let mut offset = self.tokens.last().map_or(0, |t| t.end_byte);
            let mut result = vec![];
            for v in values {
                let data = bytes(v)?;
                let mut alternatives = vec![];
                if let Some(top) = v.get("top_logprobs").filter(|v| !v.is_null()) {
                    let top = top.as_array()?;
                    if top.len() > ALTERNATIVES {
                        return None;
                    }
                    for candidate in top {
                        alternatives.push(Alternative {
                            bytes: bytes(candidate)?,
                            logprob: probability(candidate)?,
                        });
                    }
                    alternatives.sort_by(|a, b| b.logprob.total_cmp(&a.logprob));
                }
                let end_byte = offset.checked_add(data.len())?;
                result.push(InspectionToken {
                    start_byte: offset,
                    end_byte,
                    bytes: data,
                    logprob: probability(v)?,
                    alternatives,
                });
                offset = end_byte;
            }
            Some(result)
        })();
        match parsed {
            Some(tokens) => self.tokens.extend(tokens),
            None => {
                self.invalid = true;
                self.tokens.clear();
            }
        }
    }
    pub fn finish(self, model: &str, answer: &str, supported: bool) -> AnswerInspection {
        let reason = if !supported {
            Some("not_supported")
        } else if self.invalid {
            Some("invalid_data")
        } else if self.tokens.is_empty() {
            Some("not_recorded")
        } else if !self
            .tokens
            .iter()
            .flat_map(|t| t.bytes.iter().copied())
            .eq(answer.bytes())
        {
            Some("incomplete")
        } else {
            None
        };
        match reason {
            Some(reason) => AnswerInspection::unavailable(model, reason),
            None => AnswerInspection {
                model: model.into(),
                reason: None,
                tokens: self.tokens,
            },
        }
    }
}

/// Client request to continue a saved answer with a recorded alternative.
/// The client sends only ids; the server builds the prefix from saved bytes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContinueFrom {
    pub output_id: uuid::Uuid,
    pub token_position: usize,
    /// 1-based rank in the saved alternatives of that token.
    pub alternative_rank: usize,
}
/// Saved link from a continued answer to its source answer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct ContinuedFrom {
    pub output_id: uuid::Uuid,
    pub token_position: i32,
    pub alternative_rank: i16,
    pub source_byte: i32,
}
/// Prefix for a continuation and the recorded tokens that make it.
#[derive(Clone, Debug)]
pub struct Continuation {
    pub source: ContinueFrom,
    pub source_byte: usize,
    pub prefix: String,
    pub tokens: Vec<InspectionToken>,
}
impl Continuation {
    /// Saved tokens before `token_position`, then the chosen alternative with
    /// its own recorded probability and the recorded alternatives of that
    /// position. Returns None when the data is missing or the prefix is not
    /// valid UTF-8.
    pub fn build(source: ContinueFrom, inspection: &AnswerInspection, text: &str) -> Option<Self> {
        if inspection.reason.is_some() || !inspection.matches(text) {
            return None;
        }
        let token = inspection.tokens.get(source.token_position)?;
        let rank = source.alternative_rank.checked_sub(1)?;
        let alternative = token.alternatives.get(rank)?;
        let mut bytes = text.as_bytes()[..token.start_byte].to_vec();
        bytes.extend_from_slice(&alternative.bytes);
        let prefix = String::from_utf8(bytes).ok()?;
        if prefix.trim().is_empty() {
            return None;
        }
        let mut tokens = inspection.tokens[..source.token_position].to_vec();
        tokens.push(InspectionToken {
            start_byte: token.start_byte,
            end_byte: token.start_byte + alternative.bytes.len(),
            bytes: alternative.bytes.clone(),
            logprob: alternative.logprob,
            alternatives: token.alternatives.clone(),
        });
        Some(Self {
            source,
            source_byte: token.start_byte,
            prefix,
            tokens,
        })
    }
    /// Full answer text: prefix plus generated text.
    pub fn answer(&self, generated: &str) -> String {
        format!("{}{generated}", self.prefix)
    }
    /// Prefix tokens plus the generated tokens moved after the prefix.
    pub fn inspection(&self, generated: AnswerInspection) -> AnswerInspection {
        if generated.reason.is_some() {
            return generated;
        }
        let shift = self.prefix.len();
        let mut tokens = self.tokens.clone();
        tokens.extend(generated.tokens.into_iter().map(|t| InspectionToken {
            start_byte: t.start_byte + shift,
            end_byte: t.end_byte + shift,
            ..t
        }));
        AnswerInspection {
            model: generated.model,
            reason: None,
            tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn saved() -> (String, AnswerInspection) {
        let text = "A casa.".to_string();
        let mut capture = InspectionCapture::default();
        capture.push(Some(&json!([
            {"token":"A","logprob":-0.1,"top_logprobs":[{"token":"A","logprob":-0.1}]},
            {"token":" casa","logprob":-0.7,"top_logprobs":[{"token":" casa","logprob":-0.7},{"token":" cabana","logprob":-1.1}]},
            {"token":".","logprob":-0.01}
        ])),10);
        (text.clone(), capture.finish("fixture", &text, true))
    }
    #[test]
    fn continuation_uses_saved_bytes_and_the_recorded_alternative() {
        let (text, saved) = saved();
        let from = ContinueFrom {
            output_id: uuid::Uuid::nil(),
            token_position: 1,
            alternative_rank: 2,
        };
        let c = Continuation::build(from, &saved, &text).unwrap();
        assert_eq!(c.prefix, "A cabana");
        assert_eq!(c.source_byte, 1);
        assert_eq!(c.tokens.len(), 2);
        assert_eq!(c.tokens[1].logprob, -1.1);
        let mut capture = InspectionCapture::default();
        capture.push(Some(&json!([{"token":" velha","logprob":-0.3}])), 10);
        let generated = capture.finish("fixture", " velha", true);
        let full = c.inspection(generated);
        assert!(full.matches(&c.answer(" velha")));
    }
    #[test]
    fn continuation_rejects_bad_positions_ranks_and_missing_data() {
        let (text, saved) = saved();
        let at = |p, r| ContinueFrom {
            output_id: uuid::Uuid::nil(),
            token_position: p,
            alternative_rank: r,
        };
        assert!(Continuation::build(at(9, 1), &saved, &text).is_none());
        assert!(Continuation::build(at(1, 0), &saved, &text).is_none());
        assert!(Continuation::build(at(1, 3), &saved, &text).is_none());
        assert!(Continuation::build(at(1, 1), &saved, "other").is_none());
        let missing = AnswerInspection::unavailable("fixture", "not_recorded");
        assert!(Continuation::build(at(1, 1), &missing, &text).is_none());
    }
    #[test]
    fn raw_bytes_preserve_split_unicode_and_original_probabilities() {
        let text = "ก💡";
        let mut capture = InspectionCapture::default();
        for byte in text.bytes() {
            capture.push(Some(&json!([{"bytes":[byte],"logprob":-0.5,"top_logprobs":[{"bytes":[byte],"logprob":-0.5}]}])),20);
        }
        let result = capture.finish("fixture", text, true);
        assert!(result.reason.is_none());
        assert_eq!(result.tokens.len(), text.len());
        assert_eq!(result.tokens.last().unwrap().end_byte, text.len());
        assert_eq!(result.tokens[0].logprob, -0.5);
    }
    #[test]
    fn empty_control_tokens_and_empty_alternatives_keep_the_exact_answer() {
        let mut capture = InspectionCapture::default();
        capture.push(Some(&json!([
            {"token":"x","logprob":-0.2,"top_logprobs":[{"token":"","bytes":[],"logprob":-1.0}]},
            {"token":"","bytes":[],"logprob":-0.4}
        ])),3);
        let result = capture.finish("fixture", "x", true);
        assert!(result.reason.is_none());
        assert_eq!(result.tokens[1].start_byte, result.tokens[1].end_byte);
    }
    #[test]
    fn absent_mismatched_invalid_and_excessive_data_are_unavailable() {
        assert_eq!(
            InspectionCapture::default()
                .finish("fixture", "x", true)
                .reason
                .as_deref(),
            Some("not_recorded")
        );
        for (value, limit, reason) in [
            (json!([{"token":"x","logprob":0.1}]), 10, "invalid_data"),
            (json!([{"token":"x","logprob":-1}]), 0, "invalid_data"),
            (json!([{"token":"y","logprob":-1}]), 10, "incomplete"),
            (json!([{"token":"x","logprob":"bad"}]), 10, "invalid_data"),
            (
                json!([{"token":"x","bytes":[300],"logprob":-1}]),
                10,
                "invalid_data",
            ),
        ] {
            let mut capture = InspectionCapture::default();
            capture.push(Some(&value), limit);
            let result = capture.finish("fixture", "x", true);
            assert_eq!(result.reason.as_deref(), Some(reason));
            assert!(result.tokens.is_empty());
        }
    }
}
