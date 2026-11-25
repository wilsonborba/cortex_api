use crate::{
    core::process_numbers::ProcessNumber,
    domain::models::{
        context::{AssembledContext, ContextText},
        driver::{DriverError, OutputModel},
        run_record::ModelCall,
    },
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct TokenBudget {
    limit: u64,
    state: Mutex<TokenBudgetState>,
}
#[derive(Default)]
struct TokenBudgetState {
    used: u64,
    reserved: HashMap<uuid::Uuid, u64>,
}
impl TokenBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            state: Mutex::new(TokenBudgetState::default()),
        }
    }
    pub fn remaining(&self) -> u64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.limit
            .saturating_sub(state.used)
            .saturating_sub(state.reserved.values().copied().sum::<u64>())
    }
    pub fn charge(&self, amount: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(next) = state.used.checked_add(amount) else {
            return false;
        };
        if next.saturating_add(state.reserved.values().copied().sum::<u64>()) > self.limit {
            return false;
        }
        state.used = next;
        true
    }
    pub fn used(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).used
    }
    pub fn settlement_usage(&self) -> u64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .used
            .saturating_add(state.reserved.values().copied().sum::<u64>())
    }
    pub fn reserve(&self, child_id: uuid::Uuid, amount: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = state.reserved.get(&child_id) {
            return *existing == amount;
        }
        let reserved = state.reserved.values().copied().sum::<u64>();
        if state.used.saturating_add(reserved).saturating_add(amount) > self.limit {
            return false;
        }
        state.reserved.insert(child_id, amount);
        true
    }
    pub fn settle_reservation(&self, child_id: uuid::Uuid, used: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(allocation) = state.reserved.get(&child_id).copied() else {
            return false;
        };
        if used > allocation {
            return false;
        }
        let other_reserved = state
            .reserved
            .values()
            .copied()
            .sum::<u64>()
            .saturating_sub(allocation);
        if state
            .used
            .saturating_add(used)
            .saturating_add(other_reserved)
            > self.limit
        {
            return false;
        }
        state.reserved.remove(&child_id);
        state.used = state.used.saturating_add(used);
        true
    }
}

pub struct ThinkingInput<'a> {
    pub model: &'a OutputModel,
    pub context: &'a AssembledContext,
    pub images: Vec<String>,
    /// Continuation prefix for the answer call only.
    pub prefix: Option<String>,
    pub tools_enabled: bool,
    pub budget: Option<Arc<TokenBudget>>,
}
#[derive(Default)]
pub struct ThinkingResult {
    pub notes: Vec<ContextText>,
    pub text: String,
    pub calls: Vec<ModelCall>,
    pub notice: Option<ProcessNumber>,
    pub error: Option<DriverError>,
}
