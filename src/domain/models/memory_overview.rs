//! Memory overview (cortex_api #121): data for a readable memory screen.
//! The rules here are pure; the service builds them from Hippocampus and the
//! Cortex chat tables. The API sends numbers, ids and labels, no sentences.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Query of `GET /memory-graph/overview`.
#[derive(Debug, Deserialize)]
pub struct OverviewQuery {
    pub app_id: String,
}

/// Query of `GET /memory-graph/items`.
#[derive(Debug, Deserialize)]
pub struct ItemsQuery {
    pub app_id: String,
    #[serde(default)]
    pub source: SourceFilter,
    pub group_id: Option<String>,
    #[serde(default)]
    pub period: Period,
    pub q: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceFilter {
    #[default]
    All,
    Chat,
    File,
    Note,
    Person,
    Topic,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Period {
    /// The last 24 hours: the server does not know the local midnight.
    #[serde(rename = "today")]
    Today,
    #[serde(rename = "7d")]
    Days7,
    #[serde(rename = "30d")]
    Days30,
    #[default]
    #[serde(rename = "all")]
    All,
}

impl Period {
    /// The oldest `created_at` that passes the filter, or none for `all`.
    pub fn since(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::Today => Some(now - Duration::hours(24)),
            Self::Days7 => Some(now - Duration::days(7)),
            Self::Days30 => Some(now - Duration::days(30)),
            Self::All => None,
        }
    }
}

/// Where a memory came from (rule 1 of `/items`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Chat,
    File,
    Note,
}

impl SourceKind {
    /// `chat` when the memory is the exchange memory of a chat output; `file`
    /// when it is a chat input file memory or its type is `file`; else `note`.
    pub fn of(is_chat_output: bool, is_input_file: bool, memory_type: &str) -> Self {
        if is_chat_output {
            Self::Chat
        } else if is_input_file || memory_type == "file" {
            Self::File
        } else {
            Self::Note
        }
    }

    /// Facet label of the example in the issue.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chats",
            Self::File => "Files",
            Self::Note => "Saved notes",
        }
    }
}

/// Plain name of a tag (rule against raw storage names): `namespace:value`
/// becomes "Namespace: Value" with `-` and `_` as spaces; `general:` tags
/// show only the value.
pub fn tag_label(name: &str) -> String {
    let plain = |s: &str| s.replace(['-', '_'], " ").trim().to_owned();
    match name.split_once(':') {
        Some((namespace, value)) if namespace.trim().eq_ignore_ascii_case("general") => {
            plain(value)
        }
        Some((namespace, value)) if !namespace.trim().is_empty() => {
            let namespace = plain(namespace);
            let mut chars = namespace.chars();
            let head = chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
                .unwrap_or_default();
            format!("{head}{}: {}", chars.as_str(), plain(value))
        }
        Some((_, value)) => plain(value),
        None => plain(name),
    }
}

/// Plain name of a memory type. The issue gives "Conversation" for
/// `episodic`; other types show their own name with a capital letter.
pub fn type_label(memory_type: &str) -> String {
    if memory_type == "episodic" {
        return "Conversation".into();
    }
    let plain = memory_type.replace(['-', '_'], " ");
    let mut chars = plain.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// Title key for the merge rule: lowercase, trimmed, spaces collapsed.
pub fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The first 160 characters of the summary, or of the content when there is
/// no summary.
pub fn short_summary(summary: &str, content: &str) -> String {
    let text = if summary.trim().is_empty() {
        content
    } else {
        summary
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().take(160).collect()
}

/// Opaque paging cursor: base64 of the last `created_at` and `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: String,
}

impl Cursor {
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(format!("{}|{}", self.created_at.to_rfc3339(), self.id))
    }

    pub fn decode(raw: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let (at, id) = text.split_once('|')?;
        let created_at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
        (!id.is_empty()).then(|| Self {
            created_at,
            id: id.to_owned(),
        })
    }
}

/// Connection kinds of a memory relation; any other type is `related_to`.
pub fn relation_kind(edge_type: &str) -> &'static str {
    match edge_type {
        "supersedes" => "supersedes",
        "corrects" => "corrects",
        "derived_from" => "derived_from",
        "supports" => "supports",
        "contradicts" => "contradicts",
        "part_of" => "part_of",
        _ => "related_to",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn tag_label_rule() {
        assert_eq!(tag_label("project:cortex"), "Project: cortex");
        assert_eq!(tag_label("project:cortex_api"), "Project: cortex api");
        assert_eq!(tag_label("work-area:deploy-fix"), "Work area: deploy fix");
        assert_eq!(tag_label("general:reading"), "reading");
        assert_eq!(tag_label("General:reading_list"), "reading list");
        assert_eq!(tag_label("plain_tag"), "plain tag");
        assert_eq!(tag_label(":value"), "value");
    }

    #[test]
    fn source_kind_rule() {
        assert_eq!(SourceKind::of(true, true, "file"), SourceKind::Chat);
        assert_eq!(SourceKind::of(false, true, "episodic"), SourceKind::File);
        assert_eq!(SourceKind::of(false, false, "file"), SourceKind::File);
        assert_eq!(SourceKind::of(false, false, "semantic"), SourceKind::Note);
    }

    #[test]
    fn title_and_type_rules() {
        assert_eq!(normalize_title("  Fix  the\tDeploy "), "fix the deploy");
        assert_eq!(normalize_title("ola"), normalize_title(" OLA "));
        assert_eq!(type_label("episodic"), "Conversation");
        assert_eq!(type_label("semantic"), "Semantic");
        assert_eq!(type_label("long_term"), "Long term");
        assert_eq!(short_summary("", "a  b\nc"), "a b c");
        assert_eq!(short_summary(&"x".repeat(300), "").chars().count(), 160);
        assert_eq!(relation_kind("supersedes"), "supersedes");
        assert_eq!(relation_kind("something_new"), "related_to");
    }

    #[test]
    fn cursor_round_trip() {
        let cursor = Cursor {
            created_at: Utc.with_ymd_and_hms(2026, 11, 14, 7, 2, 0).unwrap(),
            id: "memory|with|bars".into(),
        };
        let raw = cursor.encode();
        assert!(!raw.contains('|'));
        let back = Cursor::decode(&raw).unwrap();
        assert_eq!(back.created_at, cursor.created_at);
        // Only the first bar separates the time from the id.
        assert_eq!(back.id, "memory|with|bars");
        assert!(Cursor::decode("not base64 !").is_none());
        assert!(Cursor::decode(&URL_SAFE_NO_PAD.encode("no-separator")).is_none());
    }

    #[test]
    fn period_filter() {
        let now = Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).unwrap();
        assert_eq!(Period::Today.since(now), Some(now - Duration::hours(24)));
        assert_eq!(Period::Days7.since(now), Some(now - Duration::days(7)));
        assert_eq!(Period::Days30.since(now), Some(now - Duration::days(30)));
        assert_eq!(Period::All.since(now), None);
        let parsed: ItemsQuery =
            serde_json::from_value(serde_json::json!({"app_id":"cortex","period":"7d"})).unwrap();
        assert_eq!(parsed.period, Period::Days7);
        assert_eq!(parsed.source, SourceFilter::All);
    }
}
