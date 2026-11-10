use crate::core::process_numbers::{self, ProcessNumber};
#[derive(Debug, Clone, Copy)]
pub struct ClassifierUnavailable;
pub trait InjectionClassifier: Send + Sync {
    /// Highest malicious probability over all 512-token windows.
    fn probability(&self, text: &str) -> Result<f32, ClassifierUnavailable>;
}
#[derive(Debug, Clone, Copy)]
pub enum ShieldError {
    Blocked,
    Unavailable,
}
impl ShieldError {
    pub fn process(self) -> ProcessNumber {
        match self {
            Self::Blocked => process_numbers::SECURITY_BLOCK,
            Self::Unavailable => process_numbers::SECURITY_UNAVAILABLE,
        }
    }
}
