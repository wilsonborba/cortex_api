//! Memory overview service (cortex_api #121). One Hippocampus scope graph
//! call (depth 1) and one SQL query give an index of the caller's memories.
//! The overview, the item pages and the node detail are computed from it.
use crate::{
    dal::local::memory_records::MemoryRecord,
    domain::{
        models::{
            cache::{CacheKey, Lifetime},
            memory_graph::GraphError,
            memory_overview::{
                Cursor, ItemsQuery, SourceFilter, SourceKind, normalize_title, relation_kind,
                short_summary, tag_label, type_label,
            },
            storage::AppUser,
        },
        services::memory_graph::MemoryGraph,
    },
};
use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

/// One group of `/items`: title, count, first and last date.
type GroupRow<'a> = (&'a str, usize, DateTime<Utc>, DateTime<Utc>);

/// Default and largest page size of `/items`.
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
/// Topics and people in the overview facets.
const TOP_FACETS: usize = 10;
/// Answers listed in the node detail.
const DETAIL_USAGE: usize = 10;
/// Neighbors in the node detail map, depth 1.
const MAP_NEIGHBORS: usize = 12;

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub kind: SourceKind,
    pub id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub session_id: String,
    pub chat_title: String,
    pub output_id: String,
    pub used_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Memory {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub memory_type: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    pub source: Source,
    pub topics: Vec<Label>,
    pub people: Vec<Label>,
    pub files: Vec<Label>,
    /// Memory relations: (edge type, other memory id).
    pub relations: Vec<(String, String)>,
    /// Answers that used this memory, newest first.
    pub usage: Vec<Usage>,
}

impl Memory {
    fn size_bytes(&self) -> usize {
        self.content.len()
    }

    fn words(&self) -> usize {
        self.content.split_whitespace().count()
    }

    fn key(&self) -> (String, SourceKind, Option<String>) {
        (
            normalize_title(&self.title),
            self.source.kind,
            self.source.id.clone(),
        )
    }
}

/// The caller's memories, newest first, with plain labels.
pub struct Index {
    pub memories: Vec<Memory>,
    pub truncated: bool,
    /// Hippocampus graph nodes by id, for the detail map.
    nodes: HashMap<String, Value>,
    edges: Vec<Value>,
}

fn time(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

/// Plain label of a non-memory node. Ids are never used as labels.
fn node_label(node: &Value) -> String {
    let label = node["label"].as_str().unwrap_or_default();
    match node["node_type"].as_str() {
        Some("tag") => tag_label(label),
        Some("chat") if label == node["id"].as_str().unwrap_or_default() => String::new(),
        _ => label.to_owned(),
    }
}

impl Index {
    /// Builds the index from one Hippocampus scope graph and the SQL rows.
    pub fn build(graph: &Value, records: &[MemoryRecord]) -> Self {
        let nodes: HashMap<String, Value> = graph["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|n| Some((n["id"].as_str()?.to_owned(), n.clone())))
            .collect();
        let edges: Vec<Value> = graph["edges"].as_array().cloned().unwrap_or_default();
        let mut chat_of: HashMap<&str, &MemoryRecord> = HashMap::new();
        let mut file_of: HashMap<&str, &MemoryRecord> = HashMap::new();
        let mut used: HashMap<&str, Vec<Usage>> = HashMap::new();
        for r in records {
            match r.kind.as_str() {
                "chat" => {
                    chat_of.insert(&r.memory_id, r);
                }
                "file" => {
                    file_of.insert(&r.memory_id, r);
                }
                "used" => used.entry(&r.memory_id).or_default().push(Usage {
                    session_id: r.session_id.clone(),
                    chat_title: r.ref_title.clone(),
                    output_id: r.output_id.clone().unwrap_or_default(),
                    used_at: r.at,
                }),
                _ => {}
            }
        }
        let mut links: HashMap<&str, Vec<(&str, &Value)>> = HashMap::new();
        for e in &edges {
            let (Some(a), Some(b)) = (e["source_id"].as_str(), e["target_id"].as_str()) else {
                continue;
            };
            let kind = e["edge_type"].as_str().unwrap_or_default();
            if let Some(target) = nodes.get(b) {
                links.entry(a).or_default().push((kind, target));
            }
            // Memory relations count for both ends.
            if let (Some(source), Some(t)) = (nodes.get(a), nodes.get(b))
                && source["node_type"] == "memory"
                && t["node_type"] == "memory"
            {
                links.entry(b).or_default().push((kind, source));
            }
        }
        let mut memories: Vec<Memory> = nodes
            .values()
            .filter(|n| n["node_type"] == "memory")
            .map(|n| {
                let id = n["id"].as_str().unwrap_or_default().to_owned();
                let m = &n["metadata"];
                let memory_type = text(&m["memory_type"]);
                let mut topics = vec![];
                let mut people = vec![];
                let mut files = vec![];
                let mut relations = vec![];
                for (kind, target) in links.get(id.as_str()).into_iter().flatten() {
                    let target_id = text(&target["id"]);
                    let label = Label {
                        id: target_id.clone(),
                        label: node_label(target),
                    };
                    match target["node_type"].as_str() {
                        Some("tag") => topics.push(label),
                        Some("entity") => people.push(label),
                        Some("file") => files.push(label),
                        Some("memory") if target_id != id => {
                            relations.push(((*kind).to_owned(), target_id))
                        }
                        _ => {}
                    }
                }
                for list in [&mut topics, &mut people, &mut files] {
                    list.sort_by(|a, b| a.label.cmp(&b.label));
                    list.dedup_by(|a, b| a.id == b.id);
                }
                let title = m["title"]
                    .as_str()
                    .or(n["label"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                let chat = chat_of.get(id.as_str());
                let file = file_of.get(id.as_str());
                let kind = SourceKind::of(chat.is_some(), file.is_some(), &memory_type);
                let source = match (kind, chat, file) {
                    (SourceKind::Chat, Some(c), _) => Source {
                        kind,
                        id: Some(c.ref_id.clone()),
                        title: Some(c.ref_title.clone()),
                    },
                    (SourceKind::File, _, Some(f)) => Source {
                        kind,
                        id: Some(f.ref_id.clone()),
                        title: Some(
                            files
                                .first()
                                .map(|l| l.label.clone())
                                .unwrap_or_else(|| title.clone()),
                        ),
                    },
                    (SourceKind::File, _, None) => Source {
                        kind,
                        id: files.first().map(|l| l.id.clone()),
                        title: Some(
                            files
                                .first()
                                .map(|l| l.label.clone())
                                .unwrap_or_else(|| title.clone()),
                        ),
                    },
                    _ => Source {
                        kind: SourceKind::Note,
                        id: None,
                        title: None,
                    },
                };
                let mut usage = used.remove(id.as_str()).unwrap_or_default();
                usage.sort_by(|a, b| b.used_at.cmp(&a.used_at));
                Memory {
                    title,
                    summary: text(&m["summary"]),
                    content: text(&m["content"]),
                    status: m["status"].as_str().unwrap_or("active").to_owned(),
                    created_at: time(&m["created_at"]).unwrap_or_default(),
                    updated_at: time(&m["updated_at"]),
                    memory_type,
                    source,
                    topics,
                    people,
                    files,
                    relations,
                    usage,
                    id,
                }
            })
            .collect();
        memories.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Self {
            memories,
            truncated: graph["truncated"].as_bool().unwrap_or(false),
            nodes,
            edges,
        }
    }

    /// `GET /memory-graph/overview`: numbers only, no sentence.
    pub fn overview(&self, now: DateTime<Utc>) -> Value {
        let week = now - Duration::days(7);
        let mut chats = HashSet::new();
        let mut sources: BTreeMap<SourceKind, usize> = BTreeMap::new();
        let mut types: BTreeMap<String, usize> = BTreeMap::new();
        let mut topics: HashMap<&str, (&str, usize)> = HashMap::new();
        let mut people: HashMap<&str, (&str, usize)> = HashMap::new();
        let mut used_week = HashSet::new();
        let mut last_used: Option<DateTime<Utc>> = None;
        for m in &self.memories {
            *sources.entry(m.source.kind).or_default() += 1;
            *types.entry(m.memory_type.clone()).or_default() += 1;
            if m.source.kind == SourceKind::Chat
                && let Some(id) = &m.source.id
            {
                chats.insert(id.as_str());
            }
            for t in &m.topics {
                topics.entry(&t.id).or_insert((&t.label, 0)).1 += 1;
            }
            for p in &m.people {
                people.entry(&p.id).or_insert((&p.label, 0)).1 += 1;
            }
            for u in &m.usage {
                if u.used_at >= week {
                    used_week.insert(m.id.as_str());
                }
                last_used = last_used.max(Some(u.used_at));
            }
        }
        let count = |kind| sources.get(&kind).copied().unwrap_or(0);
        let top = |map: HashMap<&str, (&str, usize)>| {
            let mut list: Vec<_> = map.into_iter().collect();
            list.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.1.0.cmp(b.1.0)));
            list.into_iter()
                .take(TOP_FACETS)
                .map(|(id, (label, count))| json!({"id":id,"label":label,"count":count}))
                .collect::<Vec<_>>()
        };
        let mut months = vec![];
        for back in (0..12).rev() {
            let index = now.year() * 12 + now.month0() as i32 - back;
            let (year, month) = (index.div_euclid(12), index.rem_euclid(12) as u32 + 1);
            let start = Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap();
            let next = index + 1;
            let end = Utc
                .with_ymd_and_hms(
                    next.div_euclid(12),
                    next.rem_euclid(12) as u32 + 1,
                    1,
                    0,
                    0,
                    0,
                )
                .unwrap();
            let count = self
                .memories
                .iter()
                .filter(|m| m.created_at >= start && m.created_at < end)
                .count();
            months.push(json!({"month": format!("{year:04}-{month:02}"), "count": count}));
        }
        json!({
            "summary": {
                "memories": self.memories.len(),
                "size_bytes": self.memories.iter().map(Memory::size_bytes).sum::<usize>(),
                "chats": chats.len(),
                "files": count(SourceKind::File),
                "people": people.len(),
                "topics": topics.len(),
                "notes": count(SourceKind::Note),
                "first_at": self.memories.last().map(|m| m.created_at),
                "last_at": self.memories.first().map(|m| m.created_at),
                "added_7_days": self.memories.iter().filter(|m| m.created_at >= week).count(),
                "used_7_days": used_week.len(),
                "last_used_at": last_used,
            },
            "facets": {
                "sources": sources.iter().map(|(kind, count)| json!({"kind":kind,"label":kind.label(),"count":count})).collect::<Vec<_>>(),
                "types": types.iter().map(|(t, count)| json!({"type":t,"label":type_label(t),"count":count})).collect::<Vec<_>>(),
                "months": months,
                "topics": top(topics),
                "people": top(people),
            },
            "truncated": self.truncated,
        })
    }

    fn item(&self, m: &Memory, stack: &[&Memory]) -> Value {
        let labels = |list: &[Label]| {
            list.iter()
                .map(|l| json!({"id":l.id,"label":l.label}))
                .collect::<Vec<_>>()
        };
        json!({
            "id": m.id,
            "title": m.title,
            "summary": short_summary(&m.summary, &m.content),
            "type": m.memory_type,
            "type_label": type_label(&m.memory_type),
            "source": {"kind": m.source.kind, "id": m.source.id, "title": m.source.title},
            "created_at": m.created_at,
            "updated_at": m.updated_at,
            "size_bytes": m.size_bytes(),
            "words": m.words(),
            "used_in_answers": m.usage.len(),
            "last_used_at": m.usage.first().map(|u| u.used_at),
            "status": m.status,
            "topics": labels(&m.topics),
            "people": labels(&m.people),
            "files": m.files.iter().map(|l| json!({"id":l.id,"name":l.label})).collect::<Vec<_>>(),
            "stack": (stack.len() > 1).then(|| json!({
                "count": stack.len(),
                "ids": stack.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
                "first_at": stack.last().map(|s| s.created_at),
                "last_at": stack.first().map(|s| s.created_at),
            })),
        })
    }

    /// The group a memory belongs to for a source filter.
    fn groups_of(m: &Memory, source: SourceFilter) -> Vec<(&str, &str)> {
        match source {
            SourceFilter::Chat | SourceFilter::File
                if m.source.kind
                    == if source == SourceFilter::Chat {
                        SourceKind::Chat
                    } else {
                        SourceKind::File
                    } =>
            {
                m.source
                    .id
                    .as_deref()
                    .map(|id| (id, m.source.title.as_deref().unwrap_or_default()))
                    .into_iter()
                    .collect()
            }
            SourceFilter::Person => m
                .people
                .iter()
                .map(|l| (l.id.as_str(), l.label.as_str()))
                .collect(),
            SourceFilter::Topic => m
                .topics
                .iter()
                .map(|l| (l.id.as_str(), l.label.as_str()))
                .collect(),
            _ => vec![],
        }
    }

    /// `GET /memory-graph/items`: one page, newest first, merged and labeled.
    pub fn items(&self, query: &ItemsQuery, now: DateTime<Utc>) -> Result<Value, GraphError> {
        let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
        if limit == 0 || limit > MAX_LIMIT {
            return Err(GraphError::Invalid);
        }
        let cursor = match query.cursor.as_deref().filter(|c| !c.is_empty()) {
            Some(raw) => Some(Cursor::decode(raw).ok_or(GraphError::Invalid)?),
            None => None,
        };
        let since = query.period.since(now);
        let needle = query
            .q
            .as_deref()
            .map(|q| q.trim().to_lowercase())
            .filter(|q| !q.is_empty());
        let group = query.group_id.as_deref().filter(|g| !g.is_empty());
        let filtered: Vec<&Memory> = self
            .memories
            .iter()
            .filter(|m| since.is_none_or(|s| m.created_at >= s))
            .filter(|m| match query.source {
                SourceFilter::All => true,
                SourceFilter::Chat => m.source.kind == SourceKind::Chat,
                SourceFilter::File => m.source.kind == SourceKind::File,
                SourceFilter::Note => m.source.kind == SourceKind::Note,
                SourceFilter::Person => !m.people.is_empty(),
                SourceFilter::Topic => !m.topics.is_empty(),
            })
            .filter(|m| {
                group.is_none_or(|g| {
                    Self::groups_of(m, query.source)
                        .iter()
                        .any(|(id, _)| *id == g)
                })
            })
            .filter(|m| {
                needle.as_deref().is_none_or(|n| {
                    m.title.to_lowercase().contains(n)
                        || m.summary.to_lowercase().contains(n)
                        || m.topics
                            .iter()
                            .chain(&m.people)
                            .chain(&m.files)
                            .any(|l| l.label.to_lowercase().contains(n))
                })
            })
            .collect();
        let total = filtered.len();
        let grouped = matches!(
            query.source,
            SourceFilter::Chat | SourceFilter::File | SourceFilter::Person | SourceFilter::Topic
        );
        if grouped && group.is_none() {
            let mut groups: HashMap<&str, GroupRow> = HashMap::new();
            for m in &filtered {
                for (id, title) in Self::groups_of(m, query.source) {
                    let g = groups
                        .entry(id)
                        .or_insert((title, 0, m.created_at, m.created_at));
                    g.1 += 1;
                    g.2 = g.2.min(m.created_at);
                    g.3 = g.3.max(m.created_at);
                }
            }
            let mut list: Vec<_> = groups.into_iter().collect();
            list.sort_by(|a, b| b.1.3.cmp(&a.1.3).then(a.0.cmp(b.0)));
            let kind = match query.source {
                SourceFilter::Chat => "chat",
                SourceFilter::File => "file",
                SourceFilter::Person => "person",
                _ => "topic",
            };
            return Ok(json!({
                "items": [],
                "groups": list.iter().map(|(id, (title, count, first, last))| json!({
                    "id": id, "kind": kind, "title": title, "count": count,
                    "first_at": first, "last_at": last,
                })).collect::<Vec<_>>(),
                "next_cursor": null,
                "total": total,
            }));
        }
        let start = cursor.as_ref().map_or(0, |c| {
            filtered
                .iter()
                .position(|m| (m.created_at, m.id.as_str()) < (c.created_at, c.id.as_str()))
                .unwrap_or(filtered.len())
        });
        // Merge rule: inside one page, the same normalized title and the same
        // source become one item with a stack; the item shows the newest one.
        let mut page: Vec<Vec<&Memory>> = vec![];
        let mut keys: HashMap<(String, SourceKind, Option<String>), usize> = HashMap::new();
        let mut consumed = start;
        for m in &filtered[start..] {
            match keys.get(&m.key()) {
                Some(&at) => page[at].push(m),
                None if page.len() == limit => break,
                None => {
                    keys.insert(m.key(), page.len());
                    page.push(vec![m]);
                }
            }
            consumed += 1;
        }
        let next_cursor = (consumed < filtered.len()).then(|| {
            let last = filtered[consumed - 1];
            Cursor {
                created_at: last.created_at,
                id: last.id.clone(),
            }
            .encode()
        });
        Ok(json!({
            "items": page.iter().map(|stack| self.item(stack[0], stack)).collect::<Vec<_>>(),
            "groups": [],
            "next_cursor": next_cursor,
            "total": total,
        }))
    }

    /// New fields of `GET /memory-graph/nodes/{node_id}` for a memory node.
    pub fn detail(&self, id: &str) -> Option<Value> {
        let m = self.memories.iter().find(|m| m.id == id)?;
        let target = |id: &str, label: &str, node_type: &str| json!({"id": id, "label": label, "node_type": node_type});
        let mut connections = vec![];
        if m.source.kind == SourceKind::Chat
            && let Some(session) = &m.source.id
        {
            connections.push(json!({
                "kind": "source_chat",
                "target": target(session, m.source.title.as_deref().unwrap_or_default(), "chat"),
            }));
        }
        for f in &m.files {
            connections.push(json!({"kind":"file","target":target(&f.id, &f.label, "file")}));
        }
        for p in &m.people {
            connections.push(json!({"kind":"person","target":target(&p.id, &p.label, "entity")}));
        }
        for t in &m.topics {
            connections.push(json!({"kind":"topic","target":target(&t.id, &t.label, "tag")}));
        }
        for (edge, other) in &m.relations {
            let label = self
                .memories
                .iter()
                .find(|o| &o.id == other)
                .map(|o| o.title.as_str())
                .unwrap_or_default();
            connections.push(json!({
                "kind": "relation",
                "relation_type": relation_kind(edge),
                "target": target(other, label, "memory"),
            }));
        }
        let title = normalize_title(&m.title);
        let same = self
            .memories
            .iter()
            .filter(|o| o.id != m.id && normalize_title(&o.title) == title)
            .count();
        if same > 0 {
            connections.push(json!({"kind":"same_title","count":same}));
        }
        Some(json!({
            "item": self.item(m, &[m]),
            "content": m.content,
            "usage": m.usage.iter().take(DETAIL_USAGE).map(|u| json!({
                "session_id": u.session_id,
                "chat_title": u.chat_title,
                "output_id": u.output_id,
                "used_at": u.used_at,
            })).collect::<Vec<_>>(),
            "connections": connections,
            "map": self.map(m),
        }))
    }

    /// The selected memory and at most 12 direct neighbors, depth 1, with
    /// plain labels.
    fn map(&self, m: &Memory) -> Value {
        let mut ids = vec![m.id.clone()];
        let mut edges = vec![];
        for e in &self.edges {
            let (a, b) = (text(&e["source_id"]), text(&e["target_id"]));
            let other = if a == m.id {
                b.clone()
            } else if b == m.id {
                a.clone()
            } else {
                continue;
            };
            if !ids.contains(&other) {
                if ids.len() > MAP_NEIGHBORS {
                    continue;
                }
                ids.push(other);
            }
            edges.push(json!({"source_id":a,"target_id":b,"edge_type":e["edge_type"]}));
        }
        let nodes: Vec<Value> = ids
            .iter()
            .filter_map(|id| self.nodes.get(id))
            .map(|n| {
                let label = if n["node_type"] == "memory" {
                    n["metadata"]["title"]
                        .as_str()
                        .or(n["label"].as_str())
                        .unwrap_or_default()
                        .to_owned()
                } else {
                    node_label(n)
                };
                json!({"id": n["id"], "node_type": n["node_type"], "label": label})
            })
            .collect();
        json!({"nodes": nodes, "edges": edges})
    }
}

impl MemoryGraph {
    fn overview_key(scope: &AppUser) -> CacheKey {
        CacheKey::MemoryOverview {
            app: scope.app_id.clone(),
            user: scope.user_id.clone(),
        }
    }

    /// One scope graph call (depth 1) and one SQL query.
    pub async fn index(&self, scope: &AppUser) -> Result<Index, GraphError> {
        let graph = self
            .hippocampus
            .graph(
                scope,
                None,
                Some(self.config.memory_overview_max_nodes),
                Some(1),
            )
            .await?;
        let records = match &self.records {
            Some(records) => records
                .load(scope)
                .await
                .map_err(|_| GraphError::Unavailable)?,
            None => vec![],
        };
        Ok(Index::build(&graph, &records))
    }

    /// The overview, cached for 30 seconds per app and user.
    pub async fn overview(&self, scope: &AppUser, now: DateTime<Utc>) -> Result<Value, GraphError> {
        let key = Self::overview_key(scope);
        if let Ok(Some(cached)) = self.cache.get(&key).await {
            return Ok(cached);
        }
        let value = self.index(scope).await?.overview(now);
        if let Ok(lifetime) = Lifetime::new(self.config.memory_overview_cache_ttl) {
            let _ = self.cache.put(&key, &value, lifetime).await;
        }
        Ok(value)
    }

    pub async fn items(
        &self,
        scope: &AppUser,
        query: &ItemsQuery,
        now: DateTime<Utc>,
    ) -> Result<Value, GraphError> {
        self.index(scope).await?.items(query, now)
    }
}
