use crate::domain::models::{
    driver::{DriverError, OutputModel},
    run_record::{ModelCall, WebMode},
};
pub struct QueryInput<'a> {
    pub mode: WebMode,
    pub question: &'a str,
    pub model: &'a OutputModel,
    pub images: Vec<String>,
}
pub struct QueryResult {
    pub queries: Vec<String>,
    pub call: Option<ModelCall>,
    pub error: Option<DriverError>,
}
