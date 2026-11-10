use crate::{
    core::config::Config,
    domain::models::security::{ClassifierUnavailable, InjectionClassifier},
};
use ort::{ep, session::Session, value::Tensor};
use std::sync::Mutex;
use tokenizers::{Encoding, PostProcessor, Tokenizer, TruncationDirection};
/// Local Prompt Guard 2 22M export: int64 input_ids, attention_mask and optional
/// token_type_ids; float32 logits [1,2], ordered BENIGN, MALICIOUS. CPU only.
pub struct PromptGuard {
    tokenizer: Tokenizer,
    session: Mutex<Session>,
}
impl PromptGuard {
    pub fn load(config: &Config) -> Result<Self, ClassifierUnavailable> {
        let load = || -> Result<Self, Box<dyn std::error::Error>> {
            if !config.shield_model_path.is_file()
                || !config.shield_tokenizer_path.is_file()
                || !config.shield_runtime_path.is_file()
            {
                return Err("Prompt Guard files unavailable".into());
            }
            ort::init_from(&config.shield_runtime_path)?.commit();
            let mut tokenizer = Tokenizer::from_file(&config.shield_tokenizer_path)
                .map_err(|_| "Tokenizer unavailable")?;
            tokenizer.with_padding(None);
            tokenizer
                .with_truncation(None)
                .map_err(|_| "Tokenizer invalid")?;
            let session = Session::builder()?
                .with_execution_providers([ep::CPU::default().build()])?
                .with_intra_threads(config.shield_cpu_threads)?
                .commit_from_file(&config.shield_model_path)?;
            let names: Vec<_> = session.inputs().iter().map(|v| v.name()).collect();
            if !names.contains(&"input_ids")
                || !names.contains(&"attention_mask")
                || names
                    .iter()
                    .any(|n| !matches!(*n, "input_ids" | "attention_mask" | "token_type_ids"))
                || !session.outputs().iter().any(|v| v.name() == "logits")
            {
                return Err("Prompt Guard export contract mismatch".into());
            }
            Ok(Self {
                tokenizer,
                session: Mutex::new(session),
            })
        };
        load().map_err(|_| ClassifierUnavailable)
    }
    fn window(session: &mut Session, encoding: &Encoding) -> Result<f32, ClassifierUnavailable> {
        let infer = || -> ort::Result<f32> {
            let length = encoding.get_ids().len();
            if length == 0 || length > 512 {
                return Err(ort::Error::new("Invalid classifier window"));
            }
            let mut inputs = vec![];
            for input in session.inputs() {
                let values: Vec<i64> = match input.name() {
                    "input_ids" => encoding.get_ids(),
                    "attention_mask" => encoding.get_attention_mask(),
                    "token_type_ids" => encoding.get_type_ids(),
                    _ => return Err(ort::Error::new("Invalid classifier input")),
                }
                .iter()
                .map(|v| i64::from(*v))
                .collect();
                inputs.push((
                    input.name().to_owned(),
                    Tensor::from_array(([1, length], values))?,
                ));
            }
            let output = session.run(inputs)?;
            let (shape, logits) = output["logits"].try_extract_tensor::<f32>()?;
            if shape.as_ref() != [1, 2]
                || logits.len() != 2
                || logits.iter().any(|v| !v.is_finite())
            {
                return Err(ort::Error::new("Invalid classifier logits"));
            }
            Ok(malicious_probability([logits[0], logits[1]]))
        };
        let mut infer = infer;
        let probability = infer().map_err(|_| ClassifierUnavailable)?;
        Ok(probability)
    }
}
/// Stable two-class softmax. Index 1 is MALICIOUS in Prompt Guard 2.
pub fn malicious_probability(logits: [f32; 2]) -> f32 {
    let max = logits[0].max(logits[1]);
    let benign = (logits[0] - max).exp();
    let malicious = (logits[1] - max).exp();
    malicious / (benign + malicious)
}
impl InjectionClassifier for PromptGuard {
    fn probability(&self, text: &str) -> Result<f32, ClassifierUnavailable> {
        // Encode the complete input before splitting. Early tokenizer truncation
        // can discard overflow. Add the model special tokens to every window.
        let mut encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|_| ClassifierUnavailable)?;
        let special = self
            .tokenizer
            .get_post_processor()
            .map_or(0, |p| p.added_tokens(false));
        let length = 512usize
            .checked_sub(special)
            .filter(|n| *n > 64)
            .ok_or(ClassifierUnavailable)?;
        encoding.truncate(length, 64, TruncationDirection::Right);
        let mut windows = std::mem::take(encoding.get_overflowing_mut());
        windows.insert(0, encoding);
        let mut session = self.session.lock().map_err(|_| ClassifierUnavailable)?;
        let mut probability = 0.0f32;
        for mut window in windows {
            window.get_overflowing_mut().clear();
            let window = self
                .tokenizer
                .post_process(window, None, true)
                .map_err(|_| ClassifierUnavailable)?;
            probability = probability.max(Self::window(&mut session, &window)?);
        }
        Ok(probability)
    }
}
