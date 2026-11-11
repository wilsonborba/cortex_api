use std::{num::NonZeroU64, time::Duration};
#[derive(Debug, Clone)]
pub enum CacheKey {
    History(String),
    TemporaryChat(String),
    Events(String),
    Usage { user: String, hour: String },
    Timing(String),
    Loyalty(String),
    WaitQueue(String),
}
impl CacheKey {
    pub fn name(&self) -> String {
        match self {
            Self::History(id) => format!("cortex:chat:{id}:history"),
            Self::TemporaryChat(id) => format!("cortex:temp:{id}"),
            Self::Events(id) => format!("cortex:output:{id}:events"),
            Self::Usage { user, hour } => format!("cortex:usage:{user}:{hour}"),
            Self::Timing(kind) => format!("cortex:timing:{kind}"),
            Self::Loyalty(user) => format!("cortex:loyalty:{user}"),
            Self::WaitQueue(queue) => format!("cortex:queue:{queue}"),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Lifetime(NonZeroU64);
impl Lifetime {
    pub fn new(duration: Duration) -> Result<Self, CacheError> {
        let millis = u64::try_from(duration.as_millis())
            .ok()
            .filter(|n| *n <= i64::MAX as u64)
            .and_then(NonZeroU64::new)
            .ok_or(CacheError::InvalidLifetime)?;
        Ok(Self(millis))
    }
    pub fn milliseconds(self) -> u64 {
        self.0.get()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheError {
    Unavailable,
    InvalidLifetime,
}
