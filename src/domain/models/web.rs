use crate::domain::models::{context::ContextText, events::Source, pipeline::SourceNotice};
#[derive(Default)]
pub struct GatheredWeb {
    pub texts: Vec<ContextText>,
    pub sources: Vec<Source>,
    pub notices: Vec<SourceNotice>,
    pub pages_read: u32,
}
