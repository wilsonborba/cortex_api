use crate::{
    core::process_numbers,
    dal::remote::hippocampus::Hippocampus,
    domain::{
        models::{identity::Caller, personas::*, storage::AppUser},
        services::security::SecurityShield,
    },
};
use reqwest::Method;
use serde_json::{Value, json};
use std::sync::Arc;
/// Persona routes through Hippocampus scoped entities (area 23).
#[derive(Clone)]
pub struct Personas {
    pub hippocampus: Hippocampus,
    pub shield: Arc<SecurityShield>,
}
fn unavailable() -> PersonaError {
    PersonaError::Process(process_numbers::PERSONA_UNAVAILABLE)
}
impl Personas {
    fn present(&self, scope: &AppUser, value: &Value) -> Result<Persona, PersonaError> {
        // Do not trust a wrong-scope or malformed persona, even from a LAN peer.
        if value["workspace_id"] != self.hippocampus.workspace()
            || value["app_id"] != scope.app_id.as_str()
            || value["user_id"].as_str().unwrap_or_default() != scope.user_id
            || !matches!(value["entity_type"].as_str(), Some("person" | "bot"))
        {
            return Err(unavailable());
        }
        Ok(Persona {
            persona_id: value["id"].as_str().ok_or_else(unavailable)?.into(),
            kind: value["entity_type"].as_str().unwrap_or_default().into(),
            name: value["canonical_name"]
                .as_str()
                .ok_or_else(unavailable)?
                .into(),
            aliases: value["aliases"].clone(),
            is_self: value["is_self"].as_bool().unwrap_or(false),
            attributes: value["attributes"].clone(),
        })
    }
    async fn call(
        &self,
        method: Method,
        segments: &[&str],
        caller: &Caller,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value, PersonaError> {
        let (status, value) = self
            .hippocampus
            .persona_call(method, segments, &caller.storage_scope(), query, body)
            .await
            .map_err(|_| unavailable())?;
        match status {
            200..=299 => Ok(value["data"].clone()),
            404 => Err(PersonaError::Process(process_numbers::PERSONA_NOT_FOUND)),
            // Guardrail errors keep their field path (23, issue requirement 7).
            400 | 409 | 422 => {
                let error = &value["error"];
                let errors = error["details"]["errors"]
                    .as_array()
                    .filter(|e| !e.is_empty())
                    .map(|errors| {
                        errors
                            .iter()
                            .map(|e| FieldError {
                                field_path: e["field_path"].as_str().unwrap_or_default().into(),
                                message: e["message"].as_str().unwrap_or_default().into(),
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        vec![FieldError {
                            field_path: String::new(),
                            message: error["message"].as_str().unwrap_or_default().into(),
                        }]
                    });
                Err(PersonaError::Invalid(errors))
            }
            _ => Err(unavailable()),
        }
    }
    /// Saved persona text goes through the SecurityShield (requirement 8).
    async fn check(&self, value: &Value) -> Result<(), PersonaError> {
        self.shield
            .check_persona(value)
            .await
            .map_err(|e| PersonaError::Process(e.process()))
    }
    pub async fn list(
        &self,
        caller: &Caller,
        query: &PersonaQuery,
    ) -> Result<Vec<Persona>, PersonaError> {
        let mut pairs = vec![];
        if let Some(q) = &query.q {
            pairs.push(("q", q.as_str()));
        }
        if let Some(kind) = query.kind {
            pairs.push(("type", kind.name()));
        }
        let data = self
            .call(Method::GET, &["entities"], caller, &pairs, None)
            .await?;
        let scope = caller.storage_scope();
        data.as_array()
            .ok_or_else(unavailable)?
            .iter()
            .map(|v| self.present(&scope, v))
            .collect()
    }
    pub async fn create(&self, caller: &Caller, body: NewPersona) -> Result<Persona, PersonaError> {
        let value = json!({
            "entity_type": body.kind.name(),
            "canonical_name": body.name,
            "aliases": body.aliases,
            "is_self": body.is_self,
            "attributes": body.attributes,
        });
        self.check(&value).await?;
        let data = self
            .call(Method::POST, &["entities"], caller, &[], Some(value))
            .await?;
        self.present(&caller.storage_scope(), &data)
    }
    pub async fn get(&self, caller: &Caller, id: &str) -> Result<Persona, PersonaError> {
        let data = self
            .call(Method::GET, &["entities", id], caller, &[], None)
            .await?;
        self.present(&caller.storage_scope(), &data)
    }
    pub async fn update(
        &self,
        caller: &Caller,
        id: &str,
        body: PersonaChange,
    ) -> Result<Persona, PersonaError> {
        let mut value = json!({});
        if let Some(name) = body.name {
            value["canonical_name"] = name.into();
        }
        if let Some(aliases) = body.aliases {
            value["aliases"] = aliases.into();
        }
        if let Some(is_self) = body.is_self {
            value["is_self"] = is_self.into();
        }
        if let Some(attributes) = body.attributes {
            value["attributes"] = attributes;
        }
        self.check(&value).await?;
        // The persona must exist in the scope before it changes.
        self.get(caller, id).await?;
        let data = self
            .call(Method::PATCH, &["entities", id], caller, &[], Some(value))
            .await?;
        self.present(&caller.storage_scope(), &data)
    }
    pub async fn delete(&self, caller: &Caller, id: &str) -> Result<(), PersonaError> {
        self.get(caller, id).await?;
        self.call(Method::DELETE, &["entities", id], caller, &[], None)
            .await?;
        Ok(())
    }
    /// Latest published schema fields for the persona form (24.K).
    pub async fn schema(&self, caller: &Caller, kind: PersonaType) -> Result<Value, PersonaError> {
        let data = self
            .call(
                Method::GET,
                &["persona-schemas", kind.name()],
                caller,
                &[],
                None,
            )
            .await?;
        let fields = data["fields"].as_array().ok_or_else(unavailable)?;
        Ok(json!({
            "type": kind.name(),
            "version": data["version"],
            "fields": fields.iter().map(|f| json!({
                "name": f["name"],
                "type": f["type"],
                "required": f["required"].as_bool().unwrap_or(false),
                "in_context": f["in_context"].as_bool().unwrap_or(false),
            })).collect::<Vec<_>>(),
        }))
    }
}
