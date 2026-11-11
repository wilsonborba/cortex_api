use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaType {
    Person,
    Bot,
}
impl PersonaType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Bot => "bot",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaQuery {
    pub app_id: String,
    pub q: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<PersonaType>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppQuery {
    pub app_id: String,
}
/// Body of `POST /personas` (route map 10d).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPersona {
    #[serde(rename = "type")]
    pub kind: PersonaType,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub is_self: bool,
    pub attributes: Option<serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaChange {
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub is_self: Option<bool>,
    pub attributes: Option<serde_json::Value>,
}
#[derive(Serialize)]
pub struct Persona {
    pub persona_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub aliases: serde_json::Value,
    pub is_self: bool,
    pub attributes: serde_json::Value,
}
#[derive(Serialize)]
pub struct FieldError {
    pub field_path: String,
    pub message: String,
}
pub enum PersonaError {
    Process(crate::core::process_numbers::ProcessNumber),
    Invalid(Vec<FieldError>),
}
