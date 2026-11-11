use uuid::Uuid;

pub struct Exchange<'a> {
    pub session: Uuid,
    pub chat_title: &'a str,
    pub input: &'a str,
    pub output: &'a str,
    pub topic: Option<&'a str>,
}
impl Exchange<'_> {
    pub fn memory_title(&self) -> String {
        let text = self.input.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return self.chat_title.to_owned();
        }
        let mut title: String = text.chars().take(80).collect();
        if text.chars().count() > 80
            && !text.chars().nth(80).is_some_and(char::is_whitespace)
            && let Some(boundary) = title.rfind(' ')
        {
            title.truncate(boundary);
        }
        title.trim_end().to_owned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn titles_are_one_line_word_bounded_and_unicode_safe() {
        fn title(input: &str) -> String {
            Exchange {
                session: Uuid::nil(),
                chat_title: "Synthetic chat",
                input,
                output: "answer",
                topic: None,
            }
            .memory_title()
        }
        assert_eq!(title("  first\n second\tthird  "), "first second third");
        assert_eq!(title(" \n "), "Synthetic chat");
        assert_eq!(title(&format!("{} rest", "a".repeat(80))), "a".repeat(80));
        assert_eq!(
            title(&format!("{} partialword", "a".repeat(75))),
            "a".repeat(75)
        );
        assert_eq!(title(&"ก".repeat(100)).chars().count(), 80);
    }
}
