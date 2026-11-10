use crate::{core::config::Config, domain::models::run_record::RunRecord};
use serde_json::{Value, json};
#[derive(Clone, Copy)]
pub enum RunDatabase {
    Primary,
    Test,
}
#[derive(Debug, PartialEq, Eq)]
pub enum RunStoreError {
    Unavailable,
    InvalidRecord,
}
#[derive(Clone)]
pub struct RunStore {
    client: reqwest::Client,
    base: reqwest::Url,
    user: String,
    password: String,
}
impl RunStore {
    pub fn new(
        config: &Config,
        database_url: reqwest::Url,
        user: String,
        password: String,
    ) -> Result<Self, RunStoreError> {
        if !matches!(database_url.scheme(), "http" | "https") || database_url.host_str().is_none() {
            return Err(RunStoreError::Unavailable);
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(config.document_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| RunStoreError::Unavailable)?,
            base: database_url,
            user,
            password,
        })
    }
    pub fn from_environment(database: RunDatabase) -> Result<Self, RunStoreError> {
        let config = Config::default();
        let name = match database {
            RunDatabase::Primary => config.run_database.clone(),
            RunDatabase::Test => config.run_test_database.clone(),
        };
        let mut base = reqwest::Url::parse(
            &std::env::var("CORTEX_COUCHDB_URL").map_err(|_| RunStoreError::Unavailable)?,
        )
        .map_err(|_| RunStoreError::Unavailable)?;
        base.path_segments_mut()
            .map_err(|_| RunStoreError::Unavailable)?
            .pop_if_empty()
            .push(&name);
        Self::new(
            &config,
            base,
            std::env::var("CORTEX_COUCHDB_USERNAME").map_err(|_| RunStoreError::Unavailable)?,
            std::env::var("CORTEX_COUCHDB_PASSWORD").map_err(|_| RunStoreError::Unavailable)?,
        )
    }
    fn url(&self, path: &[&str]) -> reqwest::Url {
        let mut url = self.base.clone();
        url.path_segments_mut().expect("database URL").extend(path);
        url
    }
    async fn request(
        &self,
        method: reqwest::Method,
        path: &[&str],
        body: Option<&Value>,
    ) -> Result<reqwest::Response, RunStoreError> {
        let mut request = self
            .client
            .request(method, self.url(path))
            .basic_auth(&self.user, Some(&self.password));
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.map_err(|_| RunStoreError::Unavailable)
    }
    async fn document(&self, path: &[&str]) -> Result<Option<Value>, RunStoreError> {
        let response = self.request(reqwest::Method::GET, path, None).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(RunStoreError::Unavailable);
        }
        Ok(Some(
            response
                .json()
                .await
                .map_err(|_| RunStoreError::Unavailable)?,
        ))
    }
    pub async fn initialize(&self) -> Result<(), RunStoreError> {
        let response = self.request(reqwest::Method::PUT, &[], None).await?;
        if !response.status().is_success()
            && response.status() != reqwest::StatusCode::PRECONDITION_FAILED
        {
            return Err(RunStoreError::Unavailable);
        }
        for (name, fields) in [
            ("user_created", json!(["user_id", "created_at"])),
            ("created", json!(["created_at"])),
        ] {
            let response = self
                .request(
                    reqwest::Method::POST,
                    &["_index"],
                    Some(&json!({"index":{"fields":fields},"name":name,"ddoc":name,"type":"json"})),
                )
                .await?;
            if !response.status().is_success() {
                return Err(RunStoreError::Unavailable);
            }
        }
        let mut design: Value =
            serde_json::from_str(include_str!("run_views.json")).expect("run view");
        if let Some(existing) = self.document(&["_design", "cortex_runs"]).await? {
            if existing["views"] == design["views"] && existing["language"] == design["language"] {
                return Ok(());
            }
            design["_rev"] = existing["_rev"].clone();
        }
        let response = self
            .request(
                reqwest::Method::PUT,
                &["_design", "cortex_runs"],
                Some(&design),
            )
            .await?;
        if !response.status().is_success() {
            return Err(RunStoreError::Unavailable);
        }
        Ok(())
    }
    pub async fn put(&self, record: &RunRecord) -> Result<(), RunStoreError> {
        let mut value = serde_json::to_value(record).map_err(|_| RunStoreError::InvalidRecord)?;
        let id = record.output_id.to_string();
        if let Some(existing) = self.document(&[&id]).await? {
            value["_rev"] = existing["_rev"].clone();
        }
        let response = self
            .request(reqwest::Method::PUT, &[&id], Some(&value))
            .await?;
        if !response.status().is_success() {
            return Err(RunStoreError::Unavailable);
        }
        Ok(())
    }
    pub async fn get(&self, id: uuid::Uuid) -> Result<Option<RunRecord>, RunStoreError> {
        let Some(mut value) = self.document(&[&id.to_string()]).await? else {
            return Ok(None);
        };
        let object = value.as_object_mut().ok_or(RunStoreError::InvalidRecord)?;
        object.remove("_id");
        object.remove("_rev");
        Ok(Some(
            serde_json::from_value(value).map_err(|_| RunStoreError::InvalidRecord)?,
        ))
    }
    pub async fn stats(&self) -> Result<Value, RunStoreError> {
        let mut url = self.url(&["_design", "cortex_runs", "_view", "stats"]);
        url.query_pairs_mut().append_pair("group", "true");
        let response = self
            .client
            .get(url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .map_err(|_| RunStoreError::Unavailable)?;
        if !response.status().is_success() {
            return Err(RunStoreError::Unavailable);
        }
        response
            .json()
            .await
            .map_err(|_| RunStoreError::Unavailable)
    }
    pub async fn remove(&self, id: uuid::Uuid) -> Result<(), RunStoreError> {
        let id = id.to_string();
        let Some(value) = self.document(&[&id]).await? else {
            return Ok(());
        };
        let mut url = self.url(&[&id]);
        url.query_pairs_mut().append_pair(
            "rev",
            value["_rev"].as_str().ok_or(RunStoreError::InvalidRecord)?,
        );
        let response = self
            .client
            .delete(url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .map_err(|_| RunStoreError::Unavailable)?;
        if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(RunStoreError::Unavailable);
        }
        Ok(())
    }
}

impl RunStore {
    pub async fn purge_before(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> Result<usize, RunStoreError> {
        let config = Config::default();
        let candidate = (cutoff + chrono::Duration::seconds(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut bookmark = String::new();
        let mut selected = vec![];
        // Read all pages before deleting. Compare typed UTC times at the boundary.
        loop {
            let body = json!({"selector":{"created_at":{"$lt":candidate}},"limit":config.run_read_batch,"bookmark":bookmark});
            let response = self
                .request(reqwest::Method::POST, &["_find"], Some(&body))
                .await?;
            if !response.status().is_success() {
                return Err(RunStoreError::Unavailable);
            }
            let page: Value = response
                .json()
                .await
                .map_err(|_| RunStoreError::Unavailable)?;
            let docs = page["docs"]
                .as_array()
                .ok_or(RunStoreError::InvalidRecord)?;
            for raw in docs {
                let mut value = raw.clone();
                let object = value.as_object_mut().ok_or(RunStoreError::InvalidRecord)?;
                let id = object.remove("_id").ok_or(RunStoreError::InvalidRecord)?;
                let revision = object.remove("_rev").ok_or(RunStoreError::InvalidRecord)?;
                let run: RunRecord =
                    serde_json::from_value(value).map_err(|_| RunStoreError::InvalidRecord)?;
                if id.as_str() != Some(run.output_id.to_string().as_str()) {
                    return Err(RunStoreError::InvalidRecord);
                }
                if run.created_at < cutoff {
                    selected.push((
                        run.output_id,
                        revision
                            .as_str()
                            .ok_or(RunStoreError::InvalidRecord)?
                            .to_owned(),
                    ));
                }
            }
            let next = page["bookmark"].as_str().unwrap_or("");
            if docs.len() < config.run_read_batch || next.is_empty() || next == bookmark {
                break;
            }
            bookmark = next.to_owned();
        }
        let mut deleted = 0;
        for (id, revision) in selected {
            let mut url = self.url(&[&id.to_string()]);
            url.query_pairs_mut().append_pair("rev", &revision);
            let response = self
                .client
                .delete(url)
                .basic_auth(&self.user, Some(&self.password))
                .send()
                .await
                .map_err(|_| RunStoreError::Unavailable)?;
            if response.status().is_success() {
                deleted += 1;
            } else if !matches!(
                response.status(),
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::CONFLICT
            ) {
                return Err(RunStoreError::Unavailable);
            }
        }
        Ok(deleted)
    }
    pub async fn records_since(
        &self,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<RunRecord>, RunStoreError> {
        let config = Config::default();
        let mut bookmark = String::new();
        let mut out = vec![];
        // Include the prior second in the index range, then compare typed UTC
        // timestamps. This handles RFC3339 fractional-second representations.
        let candidate = (since - chrono::Duration::seconds(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        loop {
            let body = json!({"selector":{"created_at":{"$gte":candidate}},"limit":config.run_read_batch,"bookmark":bookmark});
            let response = self
                .request(reqwest::Method::POST, &["_find"], Some(&body))
                .await?;
            if !response.status().is_success() {
                return Err(RunStoreError::Unavailable);
            }
            let page: Value = response
                .json()
                .await
                .map_err(|_| RunStoreError::Unavailable)?;
            let docs = page["docs"]
                .as_array()
                .ok_or(RunStoreError::InvalidRecord)?;
            for raw in docs {
                let mut value = raw.clone();
                let object = value.as_object_mut().ok_or(RunStoreError::InvalidRecord)?;
                object.remove("_id");
                object.remove("_rev");
                let run: RunRecord =
                    serde_json::from_value(value).map_err(|_| RunStoreError::InvalidRecord)?;
                if run.created_at >= since {
                    out.push(run);
                }
            }
            let next = page["bookmark"].as_str().unwrap_or("");
            if docs.len() < config.run_read_batch || next.is_empty() || next == bookmark {
                break;
            }
            bookmark = next.to_owned();
        }
        Ok(out)
    }
}

impl RunStore {
    pub async fn call_events(
        &self,
        user: Option<&str>,
        before: Option<&str>,
        limit: usize,
    ) -> Result<
        Vec<crate::domain::models::telemetry_reads::CallEvent>,
        crate::domain::models::telemetry_reads::TelemetryReadError,
    > {
        use crate::domain::models::telemetry_reads::{CallEvent, TelemetryReadError as E};
        let mut start = None;
        if let Some(before) = before {
            let (id, index) = before.rsplit_once(':').ok_or(E::InvalidCursor)?;
            let id = uuid::Uuid::parse_str(id).map_err(|_| E::InvalidCursor)?;
            let index = index.parse::<usize>().map_err(|_| E::InvalidCursor)?;
            let record = self
                .get(id)
                .await
                .map_err(|_| E::Unavailable)?
                .ok_or(E::InvalidCursor)?;
            if user.is_some_and(|u| u != record.user_id) {
                return Err(E::InvalidCursor);
            }
            let call = record.calls.get(index).ok_or(E::InvalidCursor)?;
            let previous = i64::try_from(index).map_err(|_| E::InvalidCursor)? - 1;
            start = Some(if let Some(user) = user {
                json!([user, call.started_at.timestamp_millis(), id, previous])
            } else {
                json!([call.started_at.timestamp_millis(), id, previous])
            });
        }
        let view = if user.is_some() {
            "events_by_user"
        } else {
            "events"
        };
        let mut url = self.url(&["_design", "cortex_runs", "_view", view]);
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("descending", "true")
                .append_pair("limit", &limit.to_string());
            if let Some(user) = user {
                let key = start.unwrap_or_else(|| json!([user, {}]));
                query
                    .append_pair("startkey", &key.to_string())
                    .append_pair("endkey", &json!([user]).to_string());
            } else if let Some(key) = start {
                query.append_pair("startkey", &key.to_string());
            }
        }
        let response = self
            .client
            .get(url)
            .basic_auth(&self.user, Some(&self.password))
            .send()
            .await
            .map_err(|_| E::Unavailable)?;
        if !response.status().is_success() {
            return Err(E::Unavailable);
        }
        let value: Value = response.json().await.map_err(|_| E::Unavailable)?;
        let rows = value["rows"].as_array().ok_or(E::Unavailable)?;
        rows.iter()
            .map(|row| {
                serde_json::from_value::<CallEvent>(row["value"].clone())
                    .map_err(|_| E::Unavailable)
            })
            .collect()
    }
}
