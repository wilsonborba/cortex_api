use cortex_api::{
    core::config::{Config, ContextFormat},
    domain::{
        models::context::*,
        services::context::{assemble, encode},
    },
};
fn text(label: &str, source: &str, value: &str) -> ContextText {
    ContextText {
        label: label.into(),
        source: Some(source.into()),
        text: value.into(),
    }
}
fn input() -> ContextInput {
    ContextInput {
        question: "What is the answer?".into(),
        ..Default::default()
    }
}
#[test]
fn json_and_toon_encode_the_same_data() {
    let values = vec![
        text("memory", "id", "comma, newline\nไทย \"quote\""),
        text("file", "file:1", "Português"),
    ];
    let json = encode(ContextFormat::Json, &values);
    let toon = encode(ContextFormat::Toon, &values);
    let left: serde_json::Value = serde_json::from_str(&json).unwrap();
    let right: serde_json::Value = toon_format::decode_default(&toon).unwrap();
    assert_eq!(left, right);
    for format in [ContextFormat::Json, ContextFormat::Toon] {
        let config = Config {
            context_format: format,
            ..Default::default()
        };
        let result = assemble(
            &config,
            ContextInput {
                memories: values.clone(),
                ..input()
            },
        )
        .unwrap();
        assert!(result.prompt.starts_with("Question: What is the answer?\n"));
        assert!(result.prompt.ends_with("\nQuestion: What is the answer?"));
        assert_eq!(result.system, config.system_instruction);
        assert!(result.texts == values);
    }
}
#[test]
fn files_first_share_eight_chunks_and_three_per_page() {
    let config = Config::default();
    let mut data = input();
    data.files = (0..6)
        .map(|i| {
            text(
                &format!("file{i}"),
                if i < 4 { "one:page1" } else { "one:page2" },
                "short",
            )
        })
        .collect();
    data.memories = (0..8)
        .map(|i| text(&format!("memory{i}"), "memory", "short"))
        .collect();
    data.web = (0..7)
        .map(|i| text(&format!("web{i}"), "https://page", "short"))
        .collect();
    let result = assemble(&config, data).unwrap();
    assert_eq!(
        result
            .texts
            .iter()
            .filter(|t| t.label.starts_with("file"))
            .count(),
        5
    );
    assert!(!result.texts.iter().any(|t| t.label == "file3"));
    assert_eq!(
        result
            .texts
            .iter()
            .filter(|t| t.label.starts_with("memory"))
            .count(),
        5
    );
    assert_eq!(
        result
            .texts
            .iter()
            .filter(|t| t.label.starts_with("web"))
            .count(),
        3
    );
    // Ranked ends: the first allocated item is first and the second is last.
    assert_eq!(result.texts.first().unwrap().label, "file0");
    assert_eq!(result.texts.last().unwrap().label, "file1");
}
#[test]
fn oversized_sources_never_overflow_and_history_is_whole() {
    for format in [ContextFormat::Json, ContextFormat::Toon] {
        for thinking in [false, true] {
            let config = Config {
                context_format: format,
                ..Default::default()
            };
            let mut data = input();
            data.thinking = thinking;
            data.history = vec![
                Exchange {
                    input: "new question".into(),
                    output: "new answer".into(),
                },
                Exchange {
                    input: "x".repeat(3000),
                    output: "old answer".into(),
                },
                Exchange {
                    input: "older".into(),
                    output: "older answer".into(),
                },
            ];
            data.files = vec![text("file", "page", &"ไทย😀\"\\\n".repeat(10000))];
            data.memories = vec![text("memory", "id", &"m".repeat(10000))];
            data.web = vec![text("web", "url", &"w".repeat(10000))];
            let result = assemble(&config, data).unwrap();
            assert_eq!(result.history.len(), 1);
            assert_eq!(result.history[0].output, "new answer");
            assert!(result.texts.iter().any(|t| t.label == "file"));
            assert!(!result.texts.iter().any(|t| t.label == "web"));
            assert!(
                result.estimated_input_tokens
                    + result.reserved_output_tokens
                    + result.reserved_notes_tokens
                    <= 8192
            );
            assert_eq!(
                result.estimated_input_tokens,
                config.estimate_tokens(&result.system)
                    + config.estimate_tokens(&result.prompt)
                    + config.chat_template_token_reserve
            );
        }
    }
}
#[test]
fn personas_share_one_budget_and_named_precede_memories() {
    let config = Config::default();
    let mut data = input();
    data.self_persona = Some(text("self", "self", "kind"));
    data.bot_persona = Some(text("bot", "bot", "clear"));
    data.named_personas = (0..5)
        .map(|i| text(&format!("person{i}"), "person", "brief"))
        .collect();
    data.memories = vec![text("memory", "memory", "fact")];
    let result = assemble(&config, data).unwrap();
    assert_eq!(result.system_personas.len(), 2);
    let named: Vec<_> = result
        .texts
        .iter()
        .filter(|t| t.label.starts_with("person"))
        .cloned()
        .collect();
    assert_eq!(named.len(), 3);
    assert!(
        config.estimate_tokens(&encode(config.context_format, &result.system_personas))
            + config.estimate_tokens(&encode(config.context_format, &named))
            <= 700
    );
    assert_eq!(result.texts[0].label, "person0");
}
#[test]
fn maximum_accepted_question_leaves_reserves_and_drops_optional_data() {
    let config = Config::default();
    let question = "x".repeat((config.max_input_tokens() / config.question_copies) as usize);
    let mut data = ContextInput {
        question,
        thinking: true,
        ..Default::default()
    };
    data.files = vec![text("file", "page", &"x".repeat(20000))];
    let result = assemble(&config, data).unwrap();
    assert!(
        result.estimated_input_tokens
            + result.reserved_output_tokens
            + result.reserved_notes_tokens
            <= 8192
    );
    assert_eq!(result.reserved_notes_tokens, 1024);
}
