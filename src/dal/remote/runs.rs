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
    pub fn from_environment(database: RunDatabase) -> Result<Self, RunStoreError> {
        let config = Config::default();
        let name = match database {
            RunDatabase::Primary => config.run_database,
            RunDatabase::Test => config.run_test_database,
        };
        let mut base = reqwest::Url::parse(
            &std::env::var("CORTEX_COUCHDB_URL").map_err(|_| RunStoreError::Unavailable)?,
        )
        .map_err(|_| RunStoreError::Unavailable)?;
        base.path_segments_mut()
            .map_err(|_| RunStoreError::Unavailable)?
            .pop_if_empty()
            .push(&name);
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(config.document_timeout)
                .build()
                .map_err(|_| RunStoreError::Unavailable)?,
            base,
            user: std::env::var("CORTEX_COUCHDB_USERNAME")
                .map_err(|_| RunStoreError::Unavailable)?,
            password: std::env::var("CORTEX_COUCHDB_PASSWORD")
                .map_err(|_| RunStoreError::Unavailable)?,
        })
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
