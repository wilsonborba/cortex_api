use super::usage::Share;
use serde::Serialize;
#[derive(Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}
#[derive(Serialize)]
pub struct Items {
    pub images: u64,
    pub audio: u64,
}
#[derive(Serialize)]
pub struct LoyaltyWeight {
    pub value: f64,
    pub age: Option<f64>,
    pub frequency: Option<f64>,
    pub account_days: Option<u64>,
    pub sign_in_days_90: Option<u32>,
}
#[derive(Serialize)]
pub struct UsageData {
    pub window_hours: i64,
    pub tokens: Tokens,
    pub items: Items,
    pub loyalty_weight: LoyaltyWeight,
    pub formula: &'static str,
}
impl UsageData {
    pub fn new(window_hours: i64, share: Share) -> Self {
        Self {
            window_hours,
            tokens: Tokens {
                input: share.usage.input,
                output: share.usage.output,
                total: share.usage.total(),
            },
            items: Items {
                images: share.usage.images,
                audio: share.usage.audio,
            },
            loyalty_weight: LoyaltyWeight {
                value: share.weight,
                age: share.loyalty.map(|s| s.age),
                frequency: share.loyalty.map(|s| s.frequency),
                account_days: share.loyalty.map(|s| s.account_days),
                sign_in_days_90: share.loyalty.map(|s| s.sign_in_days_90),
            },
            formula: "1 + age + frequency",
        }
    }
}
#[derive(Debug)]
pub struct UsageUnavailable;
