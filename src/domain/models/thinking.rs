use crate::{
    core::process_numbers::ProcessNumber,
    domain::models::{
        context::{AssembledContext, ContextText},
        driver::{DriverError, OutputModel},
        run_record::ModelCall,
    },
};
pub struct ThinkingInput<'a> {
    pub model: &'a OutputModel,
    pub context: &'a AssembledContext,
    pub images: Vec<String>,
}
#[derive(Default)]
pub struct ThinkingResult {
    pub notes: Vec<ContextText>,
    pub text: String,
    pub calls: Vec<ModelCall>,
    pub notice: Option<ProcessNumber>,
    pub error: Option<DriverError>,
}
