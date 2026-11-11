use serde::Serialize;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub app_id: String,
    pub user_id: Option<String>,
    pub is_admin: bool,
}
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MemoryScope {
    pub workspace_id: String,
    pub app_id: String,
    pub user_id: Option<String>,
}
impl Caller {
    pub fn memory_scope(&self) -> MemoryScope {
        MemoryScope {
            workspace_id: crate::core::config::Config::default()
                .memory_workspace
                .into(),
            app_id: self.app_id.clone(),
            user_id: self.user_id.clone(),
        }
    }
    pub fn storage_scope(&self) -> super::storage::AppUser {
        super::storage::AppUser {
            app_id: self.app_id.clone(),
            user_id: self.user_id.clone().unwrap_or_default(),
        }
    }
    pub fn balancer_identity(&self) -> String {
        self.user_id
            .clone()
            .unwrap_or_else(|| format!("app:{}", self.app_id))
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum IdentityError {
    UnknownApp,
    MissingUser,
}
