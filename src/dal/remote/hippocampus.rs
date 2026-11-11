use crate::{
    core::config::Config,
    domain::models::{
        extraction::DocumentFile,
        files::{FileContent, FileMemory},
        memory_graph::GraphError,
        recall::RecalledMemory,
        storage::AppUser,
    },
};
use serde::{Deserialize, Serialize};
#[derive(Clone)]
pub struct Hippocampus {
    client: reqwest::Client,
    base: reqwest::Url,
    workspace: String,
}
#[derive(Debug, Clone, Copy)]
pub struct HippoUnavailable;
#[derive(Serialize)]
struct ExchangeMemory<'a> {
    workspace_id: &'a str,
    app_id: &'a str,
    user_id: Option<&'a str>,
    title: &'a str,
    content: String,
}
#[derive(Serialize)]
struct Search<'a> {
    workspace_id: &'a str,
    app_id: &'a str,
    user_id: Option<&'a str>,
    text: &'a str,
    tags: Vec<&'a str>,
    expand: u8,
    limit: usize,
}
#[derive(Deserialize)]
struct SearchResult {
    memory: RecalledMemory,
}
#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<SearchResult>,
}
impl Hippocampus {
    pub async fn search(
        &self,
        scope: &AppUser,
        text: &str,
        topic: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecalledMemory>, HippoUnavailable> {
        let body = Search {
            workspace_id: &self.workspace,
            app_id: &scope.app_id,
            user_id: if scope.user_id.is_empty() {
                None
            } else {
                Some(&scope.user_id)
            },
            text,
            tags: topic.into_iter().collect(),
            expand: 0,
            limit,
        };
        let url = self
            .base
            .join("api/v1/search")
            .map_err(|_| HippoUnavailable)?;
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|_| HippoUnavailable)?;
        if !response.status().is_success() {
            return Err(HippoUnavailable);
        }
        let response: SearchResponse = response.json().await.map_err(|_| HippoUnavailable)?;
        // Do not trust a malformed or wrong-scope response, even from a LAN peer.
        if response.data.iter().any(|r| {
            r.memory.workspace_id != self.workspace
                || r.memory.app_id != scope.app_id
                || r.memory.user_id.as_deref() != body.user_id
                || r.memory.id.is_empty()
        }) {
            return Err(HippoUnavailable);
        }
        Ok(response
            .data
            .into_iter()
            .take(limit)
            .map(|r| r.memory)
            .collect())
    }
    pub fn new(config: &Config) -> Result<Self, HippoUnavailable> {
        let base = reqwest::Url::parse(&format!(
            "{}/",
            config.hippocampus_url.trim_end_matches('/')
        ))
        .map_err(|_| HippoUnavailable)?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(HippoUnavailable);
        }
        let client = reqwest::Client::builder()
            .timeout(config.hippocampus_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| HippoUnavailable)?;
        Ok(Self {
            client,
            base,
            workspace: config.memory_workspace.into(),
        })
    }
    pub async fn create_exchange(
        &self,
        scope: &AppUser,
        title: &str,
        input: &str,
        output: &str,
    ) -> Result<String, HippoUnavailable> {
        let body = ExchangeMemory {
            workspace_id: &self.workspace,
            app_id: &scope.app_id,
            user_id: if scope.user_id.is_empty() {
                None
            } else {
                Some(&scope.user_id)
            },
            title,
            content: format!("Input:\n{input}\n\nAnswer:\n{output}"),
        };
        let url = self
            .base
            .join("api/v1/memories")
            .map_err(|_| HippoUnavailable)?;
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|_| HippoUnavailable)?;
        if !response.status().is_success() {
            return Err(HippoUnavailable);
        }
        let value: serde_json::Value = response.json().await.map_err(|_| HippoUnavailable)?;
        value["data"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or(HippoUnavailable)
    }
    pub async fn graph(
        &self,
        scope: &AppUser,
        node: Option<&str>,
        limit: Option<usize>,
        depth: Option<usize>,
    ) -> Result<serde_json::Value, GraphError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| GraphError::Unavailable)?
            .pop_if_empty()
            .extend(["api", "v1", "graph"])
            .extend(node.map(|n| ["nodes", n]).into_iter().flatten());
        self.scope_query(&mut url, scope);
        {
            let mut q = url.query_pairs_mut();
            if let Some(limit) = limit {
                q.append_pair("limit", &limit.to_string());
            }
            if let Some(depth) = depth {
                q.append_pair("depth", &depth.to_string());
            }
        }
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| GraphError::Unavailable)?;
        match response.status() {
            s if s.is_success() => {}
            reqwest::StatusCode::NOT_FOUND if node.is_some() => return Err(GraphError::NotFound),
            reqwest::StatusCode::BAD_REQUEST => return Err(GraphError::Invalid),
            _ => return Err(GraphError::Unavailable),
        }
        let mut value: serde_json::Value =
            response.json().await.map_err(|_| GraphError::Unavailable)?;
        let data = value["data"].take();
        let user = (!scope.user_id.is_empty()).then_some(scope.user_id.as_str());
        // Do not trust a malformed or wrong-scope graph, even from a LAN peer.
        let nodes = data["nodes"].as_array().ok_or(GraphError::Unavailable)?;
        let in_scope = nodes.iter().all(|n| {
            n["id"].as_str().is_some_and(|id| !id.is_empty())
                && (n["node_type"] != "memory"
                    || (n["metadata"]["workspace_id"] == self.workspace
                        && n["metadata"]["app_id"] == scope.app_id.as_str()
                        && n["metadata"]["user_id"].as_str() == user))
        });
        if !in_scope || !data["edges"].is_array() || (node.is_some() && !data["node"].is_object()) {
            return Err(GraphError::Unavailable);
        }
        Ok(data)
    }
    fn scope_query(&self, url: &mut reqwest::Url, scope: &AppUser) {
        url.query_pairs_mut()
            .append_pair("workspace_id", &self.workspace)
            .append_pair("app_id", &scope.app_id)
            .append_pair("user_id", &scope.user_id);
    }
    /// One call: the file, the scope and the extracted text (8.3).
    pub async fn save_file(
        &self,
        scope: &AppUser,
        file: &DocumentFile,
        text: &str,
    ) -> Result<String, HippoUnavailable> {
        let part = reqwest::multipart::Part::bytes(file.bytes.to_vec())
            .file_name(file.name.clone())
            .mime_str(&file.content_type)
            .or_else(|_| {
                reqwest::multipart::Part::bytes(file.bytes.to_vec())
                    .file_name(file.name.clone())
                    .mime_str("application/octet-stream")
            })
            .map_err(|_| HippoUnavailable)?;
        let form = reqwest::multipart::Form::new()
            .text("workspace_id", self.workspace.clone())
            .text("app_id", scope.app_id.clone())
            .text("user_id", scope.user_id.clone())
            .text("extracted_text", text.to_owned())
            .part("file", part);
        let url = self
            .base
            .join("api/v1/files")
            .map_err(|_| HippoUnavailable)?;
        let response = self
            .client
            .post(url)
            .multipart(form)
            .send()
            .await
            .map_err(|_| HippoUnavailable)?;
        if !response.status().is_success() {
            return Err(HippoUnavailable);
        }
        let value: serde_json::Value = response.json().await.map_err(|_| HippoUnavailable)?;
        let present = |key: &str| value["data"][key].as_str().filter(|id| !id.is_empty());
        present("memory_id")
            .and(present("resource_id"))
            .map(str::to_owned)
            .ok_or(HippoUnavailable)
    }
    /// The file memory comes from the scoped graph node `resource:{file_id}`.
    /// Its content is the text extracted once at upload (8.5).
    pub async fn file_memory(
        &self,
        scope: &AppUser,
        file_id: &str,
    ) -> Result<FileMemory, GraphError> {
        let node = format!("resource:{file_id}");
        let graph = self.graph(scope, Some(&node), None, Some(0)).await?;
        let nodes = graph["nodes"].as_array().ok_or(GraphError::Unavailable)?;
        let memory = graph["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["target_id"] == node.as_str())
            .filter_map(|e| e["source_id"].as_str())
            .find_map(|id| {
                nodes
                    .iter()
                    .find(|n| n["id"] == id && n["node_type"] == "memory")
            })
            .ok_or(GraphError::Unavailable)?;
        Ok(FileMemory {
            memory_id: memory["id"].as_str().unwrap_or_default().to_owned(),
            name: graph["node"]["label"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            text: memory["metadata"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            content_type: graph["node"]["metadata"]["content_type"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        })
    }
    pub async fn file_content(
        &self,
        scope: &AppUser,
        file_id: &str,
    ) -> Result<FileContent, GraphError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| GraphError::Unavailable)?
            .pop_if_empty()
            .extend(["api", "v1", "resources", file_id, "content"]);
        self.scope_query(&mut url, scope);
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| GraphError::Unavailable)?;
        match response.status() {
            s if s.is_success() => {}
            reqwest::StatusCode::NOT_FOUND => return Err(GraphError::NotFound),
            _ => return Err(GraphError::Unavailable),
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let content_type = header(reqwest::header::CONTENT_TYPE)
            .unwrap_or_else(|| "application/octet-stream".into());
        let disposition = header(reqwest::header::CONTENT_DISPOSITION);
        let bytes = response
            .bytes()
            .await
            .map_err(|_| GraphError::Unavailable)?
            .to_vec();
        Ok(FileContent {
            content_type,
            disposition,
            bytes,
        })
    }
    /// One bulk call with the chat memory ids and the scope (19.4).
    pub async fn delete_memories(
        &self,
        scope: &AppUser,
        ids: &[String],
    ) -> Result<(), HippoUnavailable> {
        let url = self
            .base
            .join("api/v1/memories/delete")
            .map_err(|_| HippoUnavailable)?;
        let user = (!scope.user_id.is_empty()).then_some(scope.user_id.as_str());
        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "workspace_id": self.workspace,
                "app_id": scope.app_id,
                "user_id": user,
                "memory_ids": ids,
            }))
            .send()
            .await
            .map_err(|_| HippoUnavailable)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(HippoUnavailable)
        }
    }
    /// One persona call. The scope always goes in the query; a body adds it
    /// again, because the Hippocampus write routes read it there.
    pub async fn persona_call(
        &self,
        method: reqwest::Method,
        segments: &[&str],
        scope: &AppUser,
        query: &[(&str, &str)],
        body: Option<serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), HippoUnavailable> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|_| HippoUnavailable)?
            .pop_if_empty()
            .extend(["api", "v1"])
            .extend(segments);
        self.scope_query(&mut url, scope);
        url.query_pairs_mut().extend_pairs(query);
        let mut request = self.client.request(method, url);
        if let Some(mut body) = body {
            body["workspace_id"] = self.workspace.clone().into();
            body["app_id"] = scope.app_id.clone().into();
            body["user_id"] = scope.user_id.clone().into();
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|_| HippoUnavailable)?;
        let status = response.status().as_u16();
        let value = response.json().await.unwrap_or(serde_json::Value::Null);
        Ok((status, value))
    }
    pub fn workspace(&self) -> &str {
        &self.workspace
    }
}
