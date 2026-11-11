use crate::domain::models::{context::ContextText, events::Source, pipeline::SourceNotice};
#[derive(Default)]
pub struct GatheredWeb {
    pub texts: Vec<ContextText>,
    pub sources: Vec<Source>,
    pub notices: Vec<SourceNotice>,
    pub pages_read: u32,
}

pub struct WebInput<'a> {
    pub mode: super::run_record::WebMode,
    pub question: &'a str,
    pub needs_web: bool,
    pub model: &'a super::driver::OutputModel,
    pub images: Vec<String>,
}
pub struct WebResult {
    pub gathered: GatheredWeb,
    pub calls: Vec<super::run_record::ModelCall>,
    pub error: Option<super::driver::DriverError>,
}
