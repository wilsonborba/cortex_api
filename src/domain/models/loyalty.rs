use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, future::Future, pin::Pin};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct LoyaltySnapshot {
    pub weight: f64,
    pub age: f64,
    pub frequency: f64,
    pub account_days: u64,
    pub sign_in_days_90: u32,
}
impl LoyaltySnapshot {
    pub fn calculate(
        created: DateTime<Utc>,
        now: DateTime<Utc>,
        days: impl IntoIterator<Item = NaiveDate>,
    ) -> Self {
        let account_days = now.signed_duration_since(created).num_days().max(0) as u64;
        let cutoff = (now - chrono::Duration::days(90)).date_naive();
        let days: BTreeSet<_> = days
            .into_iter()
            .filter(|day| *day >= cutoff && *day <= now.date_naive())
            .collect();
        let sign_in_days_90 = days.len() as u32;
        let age = (account_days as f64 / 365.0).min(1.0);
        let frequency = (sign_in_days_90 as f64 / 90.0).min(1.0);
        Self {
            weight: 1.0 + age + frequency,
            age,
            frequency,
            account_days,
            sign_in_days_90,
        }
    }
    pub fn valid(&self) -> bool {
        let age = (self.account_days as f64 / 365.0).min(1.0);
        let frequency = (self.sign_in_days_90 as f64 / 90.0).min(1.0);
        self.weight.is_finite()
            && self.age.is_finite()
            && self.frequency.is_finite()
            && (self.age - age).abs() < 1e-9
            && (self.frequency - frequency).abs() < 1e-9
            && (self.weight - (1.0 + age + frequency)).abs() < 1e-9
    }
}
#[derive(Debug, Clone, Copy)]
pub struct LoyaltyUnavailable;
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub trait LoyaltySource: Send + Sync {
    fn fetch<'a>(
        &'a self,
        now: DateTime<Utc>,
    ) -> Pending<'a, Result<Vec<(String, LoyaltySnapshot)>, LoyaltyUnavailable>>;
}
