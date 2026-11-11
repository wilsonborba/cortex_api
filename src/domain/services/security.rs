use crate::{
    core::{config::Config, process_numbers::ProcessNumber},
    domain::models::{context::ContextText, security::*},
};
use regex::RegexSet;
use std::{sync::Arc, time::Instant};
use unicode_normalization::UnicodeNormalization;
/// English patterns adapt Vigil instruction_bypass.yar and system_instructions.yar.
/// Source: https://github.com/deadbits/vigil-llm (Apache-2.0).
/// Portuguese and Thai equivalents implement the aligned language coverage.
const RULES: &[&str] = &[
    r"\brm\s+-[a-z]*r[a-z]*f\b|\brm\s+-[a-z]*f[a-z]*r\b",
    r"\bsudo\b",
    r"\bdd\s+if\s*=",
    r"\bmkfs\b",
    r"\bchmod\s+777\b",
    r"/etc/(shadow|passwd)\b",
    r"\bshutdown\s+-h\b|\breboot\b",
    r"\b(ignore|disregard|skip|forget|neglect|overlook|omit|bypass|pay no attention to|do not follow|do not obey)\s+(your\s+|all\s+)?(prior|previous|preceding|above|foregoing|earlier|initial)?\s*(content|text|instructions?|directives?|commands?|context|conversation|inputs?|data|messages?|communication|responses?|requests?)\b",
    r"\b(show|reveal|print|repeat)\s+(me\s+)?(your|the)\s+system\s+prompt\b",
    r"\b(you are|you're|act as|become)\s+(now\s+)?dan\b",
    r"system instruction:|\[system\]\(#(assistant|context)\)|<s>\[inst\]\s*<<sys>>|<</sys>>|<\|im_start\|>\s*(assistant|system)|\{\{[#/]system~\}\}",
    r"\b(ignore|ignora|ignorar|desconsidere|desconsidera|esqueça|esquece|não siga|não obedeça)\s+(todas?\s+)?(as?\s+)?(instruções|instrucoes|ordens|mensagens|regras)\s+(anteriores|prévias|previas|acima)\b",
    r"\b(esqueça|esqueca|esquece)\s+(todas?\s+)?(as?\s+)?(regras|instruções|instrucoes)\b",
    r"\b(mostre|mostra|revele|exiba|imprima)\s+(me\s+)?(o\s+|seu\s+)?(prompt|instruções|instrucoes)\s+(do\s+|de\s+)?sistema\b",
    r"(เพิกเฉย|ละเว้น|ไม่ต้องปฏิบัติตาม|อย่าทำตาม|ลืม)(ต่อ)?(คำสั่ง|คำแนะนำ|ข้อความ|กฎ)(ก่อนหน้า|ก่อนหน้านี้|เดิม|ทั้งหมด)",
    r"(แสดง|เปิดเผย|บอก)(พรอมป์ต์|คำสั่ง)(ของ)?ระบบ",
];
pub fn normalize(text: &str) -> String {
    text.nfkc().filter(|c|!matches!(*c,'\u{00ad}'|'\u{034f}'|'\u{180e}'|'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2060}'..='\u{206f}'|'\u{feff}')).flat_map(char::to_lowercase).collect()
}
pub struct SecurityShield {
    rules: RegexSet,
    classifier: Arc<dyn InjectionClassifier>,
    threshold: f32,
}
impl SecurityShield {
    pub fn new(
        config: &Config,
        classifier: Arc<dyn InjectionClassifier>,
    ) -> Result<Self, ShieldError> {
        if !config.shield_threshold.is_finite() || !(0.0..=1.0).contains(&config.shield_threshold) {
            return Err(ShieldError::Unavailable);
        }
        Ok(Self {
            rules: RegexSet::new(RULES.iter().map(|rule| normalize(rule)))
                .map_err(|_| ShieldError::Unavailable)?,
            classifier,
            threshold: config.shield_threshold,
        })
    }
    pub async fn check(&self, text: &str) -> Result<(), ShieldError> {
        let start = Instant::now();
        let result = if self.rules.is_match(&normalize(text)) {
            Err(ShieldError::Blocked)
        } else {
            let classifier = self.classifier.clone();
            let text = text.to_owned();
            match tokio::task::spawn_blocking(move || classifier.probability(&text)).await {
                Ok(Ok(score)) if score.is_finite() && (0.0..=1.0).contains(&score) => {
                    if score >= self.threshold {
                        Err(ShieldError::Blocked)
                    } else {
                        Ok(())
                    }
                }
                _ => Err(ShieldError::Unavailable),
            }
        };
        if let Err(error) = result {
            tracing::warn!(
                error_number = error.process().number,
                time_ms = start.elapsed().as_millis() as u64,
                "Input security check failed"
            );
        } else {
            tracing::info!(
                time_ms = start.elapsed().as_millis() as u64,
                "Input security check complete"
            );
        }
        result
    }
    /// A failed source check removes only that chunk, including on classifier fault.
    pub async fn filter_chunks(
        &self,
        chunks: Vec<ContextText>,
    ) -> (Vec<ContextText>, Vec<ProcessNumber>) {
        let mut safe = vec![];
        let mut notices = vec![];
        for chunk in chunks {
            match self
                .check(&format!(
                    "{}\n{}\n{}",
                    chunk.label,
                    chunk.source.as_deref().unwrap_or(""),
                    chunk.text
                ))
                .await
            {
                Ok(()) => safe.push(chunk),
                Err(error) => notices.push(error.process()),
            }
        }
        (safe, notices)
    }
    pub async fn check_persona(&self, value: &serde_json::Value) -> Result<(), ShieldError> {
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                serde_json::Value::String(text) => self.check(text).await?,
                serde_json::Value::Array(values) => pending.extend(values),
                serde_json::Value::Object(values) => pending.extend(values.values()),
                _ => {}
            }
        }
        Ok(())
    }
}
