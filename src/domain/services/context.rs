use crate::{
    core::config::{Config, ContextFormat},
    domain::{
        models::{context::*, model_catalog::InputTooLarge},
        services::input_guard,
    },
};
use std::collections::HashMap;

/// Encodes data for the model only. HTTP service bodies stay JSON.
pub fn encode(format: ContextFormat, texts: &[ContextText]) -> String {
    match format {
        ContextFormat::Json => serde_json::to_string(texts).expect("text values serialize"),
        ContextFormat::Toon => toon_format::encode_default(&texts).expect("text values encode"),
    }
}
fn render(
    config: &Config,
    question: &str,
    system_personas: &[ContextText],
    history: &[Exchange],
    texts: &[ContextText],
) -> (String, String, u64) {
    let mut system = config.system_instruction.clone();
    if !system_personas.is_empty() {
        system.push('\n');
        system.push_str(&encode(config.context_format, system_personas));
    }
    let mut prompt = format!("Question: {question}\n");
    for exchange in history {
        prompt.push_str(&format!(
            "Input: {}\nAnswer: {}\n",
            exchange.input, exchange.output
        ));
    }
    prompt.push_str(&encode(config.context_format, texts));
    prompt.push_str(&format!("\nQuestion: {question}"));
    let tokens = config
        .estimate_tokens(&system)
        .saturating_add(config.estimate_tokens(&prompt))
        .saturating_add(config.chat_template_token_reserve);
    (system, prompt, tokens)
}
/// The highest-ranked text is first, the second is last, then alternate inward.
fn edges(texts: Vec<ContextText>) -> Vec<ContextText> {
    texts
        .iter()
        .step_by(2)
        .chain(texts.iter().skip(1).step_by(2).rev())
        .cloned()
        .collect()
}
fn add_trimmed(
    list: &mut Vec<ContextText>,
    mut candidate: ContextText,
    fits: impl Fn(&[ContextText]) -> bool,
) {
    list.push(candidate.clone());
    if fits(list) {
        return;
    }
    list.pop();
    let boundaries: Vec<usize> = candidate
        .text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(candidate.text.len()))
        .collect();
    let original = candidate.text.clone();
    let (mut low, mut high) = (0, boundaries.len());
    while low < high {
        let mid = low + (high - low) / 2;
        candidate.text = original[..boundaries[mid]].into();
        list.push(candidate.clone());
        let ok = fits(list);
        list.pop();
        if ok {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    if low > 1 {
        candidate.text = original[..boundaries[low - 1]].into();
        list.push(candidate);
    }
}
pub fn assemble(config: &Config, input: ContextInput) -> Result<AssembledContext, InputTooLarge> {
    input_guard::check(config, &input.question)?;
    let notes = if input.thinking {
        config.thinking_notes_token_reserve
    } else {
        0
    };
    let available = config
        .model_context_tokens
        .saturating_sub(config.answer_token_reserve)
        .saturating_sub(notes);
    let fits = |personas: &[ContextText], history: &[Exchange], texts: &[ContextText]| {
        render(config, &input.question, personas, history, texts).2 <= available
    };
    // Config and accepted question must fit before any optional source is added.
    if !fits(&[], &[], &[]) {
        return Err(InputTooLarge);
    }
    let mut system_personas = vec![];
    for persona in input.self_persona.into_iter().chain(input.bot_persona) {
        add_trimmed(&mut system_personas, persona, |p| {
            config.estimate_tokens(&encode(config.context_format, p))
                <= config.persona_context_tokens
                && fits(p, &[], &[])
        });
    }
    let mut history = vec![];
    for exchange in input.history {
        history.push(exchange);
        let history_tokens: u64 = history
            .iter()
            .map(|e| config.estimate_tokens(&format!("Input: {}\nAnswer: {}\n", e.input, e.output)))
            .sum();
        if history_tokens > config.history_context_tokens || !fits(&system_personas, &history, &[])
        {
            history.pop();
            break;
        }
    }
    let mut texts = vec![];
    let mut pages = HashMap::<String, usize>::new();
    let mut chunks = 0;
    for file in input.files {
        if chunks == config.context_chunk_count {
            break;
        }
        let page = file.source.clone().unwrap_or_else(|| file.label.clone());
        if pages.get(&page).copied().unwrap_or(0) >= config.chunks_per_page {
            continue;
        }
        let before = texts.len();
        add_trimmed(&mut texts, file, |t| fits(&system_personas, &history, t));
        if texts.len() > before {
            chunks += 1;
            *pages.entry(page).or_default() += 1;
        }
    }
    let file_count = texts.len();
    let system_persona_tokens =
        config.estimate_tokens(&encode(config.context_format, &system_personas));
    let mut named = vec![];
    for persona in input
        .named_personas
        .into_iter()
        .take(config.named_persona_count)
    {
        add_trimmed(&mut named, persona, |p| {
            let mut combined = texts.clone();
            combined.extend_from_slice(p);
            system_persona_tokens
                .saturating_add(config.estimate_tokens(&encode(config.context_format, p)))
                <= config.persona_context_tokens
                && config.estimate_tokens(&encode(config.context_format, p))
                    <= config.memory_context_tokens
                && fits(&system_personas, &history, &combined)
        });
    }
    texts.extend(named);
    for memory in input.memories.into_iter().take(config.memory_context_count) {
        add_trimmed(&mut texts, memory, |t| {
            config.estimate_tokens(&encode(config.context_format, &t[file_count..]))
                <= config.memory_context_tokens
                && fits(&system_personas, &history, t)
        });
    }
    for web in input.web {
        if chunks == config.context_chunk_count {
            break;
        }
        let page = web.source.clone().unwrap_or_else(|| web.label.clone());
        if pages.get(&page).copied().unwrap_or(0) >= config.chunks_per_page {
            continue;
        }
        let before = texts.len();
        add_trimmed(&mut texts, web, |t| fits(&system_personas, &history, t));
        if texts.len() > before {
            chunks += 1;
            *pages.entry(page).or_default() += 1;
        }
    }
    // Reordering does not change JSON/TOON text size for these flat records.
    let texts = edges(texts);
    let (system, prompt, estimated_input_tokens) =
        render(config, &input.question, &system_personas, &history, &texts);
    Ok(AssembledContext {
        system,
        prompt,
        system_personas,
        texts,
        history,
        estimated_input_tokens,
        reserved_output_tokens: config.answer_token_reserve,
        reserved_notes_tokens: notes,
    })
}
