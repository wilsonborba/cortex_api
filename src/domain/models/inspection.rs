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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
