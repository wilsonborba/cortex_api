use crate::{core::config::Config, domain::models::loyalty::*};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct Credentials {
    pub project_ref: String,
    pub access_token: String,
}
#[derive(Clone)]
pub struct SupabaseClient {
    client: reqwest::Client,
    base: reqwest::Url,
    project: String,
    token: String,
    config: Config,
}
#[derive(Deserialize)]
pub struct Account {
    pub user_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub last_sign_in_at: Option<DateTime<Utc>>,
    pub session_days: Vec<NaiveDate>,
}
#[derive(Deserialize)]
struct AuditDay {
    user_id: Uuid,
    signin_day: NaiveDate,
}
impl SupabaseClient {
    pub fn new(
        base: reqwest::Url,
        credentials: Credentials,
        config: Config,
    ) -> Result<Self, LoyaltyUnavailable> {
        if config.loyalty_audit_batch_users == 0
            || config.loyalty_audit_batch_users > 10
            || config.loyalty_audit_row_limit < config.loyalty_audit_batch_users * 91
        {
            return Err(LoyaltyUnavailable);
        }
        if credentials.project_ref.is_empty()
            || !credentials
                .project_ref
                .bytes()
                .all(|c| c.is_ascii_alphanumeric())
            || credentials.access_token.is_empty()
        {
            return Err(LoyaltyUnavailable);
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(config.loyalty_source_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| LoyaltyUnavailable)?,
            base,
            project: credentials.project_ref,
            token: credentials.access_token,
            config,
        })
    }
    fn url(&self, suffix: &str) -> Result<reqwest::Url, LoyaltyUnavailable> {
        self.base
            .join(&format!("v1/projects/{}/{}", self.project, suffix))
            .map_err(|_| LoyaltyUnavailable)
    }
    pub async fn accounts(
        &self,
        ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<Account>, LoyaltyUnavailable> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let ids = ids
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(",");
        let cutoff = (now - chrono::Duration::days(90)).to_rfc3339();
        let end = now.to_rfc3339();
        let query = format!(
            "SELECT u.id::text AS user_id,to_char(u.created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,to_char(u.last_sign_in_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_sign_in_at,coalesce((SELECT json_agg(DISTINCT (s.created_at AT TIME ZONE 'UTC')::date) FROM auth.sessions s WHERE s.user_id=u.id AND s.created_at>='{cutoff}'::timestamptz AND s.created_at<='{end}'::timestamptz),'[]'::json) AS session_days FROM auth.users u WHERE u.id IN ({ids})"
        );
        let response = self
            .client
            .post(self.url("database/query/read-only")?)
            .bearer_auth(&self.token)
            .json(&json!({"query":query}))
            .send()
            .await
            .map_err(|_| LoyaltyUnavailable)?;
        if !response.status().is_success() {
            return Err(LoyaltyUnavailable);
        }
        response.json().await.map_err(|_| LoyaltyUnavailable)
    }
    pub async fn audit_days(
        &self,
        ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<HashMap<Uuid, BTreeSet<NaiveDate>>, LoyaltyUnavailable> {
        let mut days: HashMap<_, BTreeSet<_>> = HashMap::new();
        for (index, batch) in ids
            .chunks(self.config.loyalty_audit_batch_users)
            .enumerate()
        {
            if index > 0 {
                tokio::time::sleep(self.config.loyalty_audit_request_spacing).await;
            }
            let ids = batch
                .iter()
                .map(|id| format!("'{id}'"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT log_attributes['auth_audit_event.actor_id'] AS user_id,toDate(timestamp) AS signin_day FROM logs WHERE source='auth_audit_logs' AND log_attributes['auth_audit_event.action']='login' AND log_attributes['auth_audit_event.actor_id'] IN ({ids}) GROUP BY user_id,signin_day LIMIT {}",
                self.config.loyalty_audit_row_limit
            );
            let mut url = self.url("analytics/endpoints/logs")?;
            url.query_pairs_mut()
                .append_pair(
                    "iso_timestamp_start",
                    &(now - chrono::Duration::days(90))
                        .format("%Y-%m-%dT%H:%M:%SZ")
                        .to_string(),
                )
                .append_pair(
                    "iso_timestamp_end",
                    &now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                )
                .append_pair("sql", &sql);
            let response = self
                .client
                .get(url)
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(|_| LoyaltyUnavailable)?;
            if !response.status().is_success() {
                return Err(LoyaltyUnavailable);
            }
            let body: serde_json::Value = response.json().await.map_err(|_| LoyaltyUnavailable)?;
            if !body["error"].is_null() {
                return Err(LoyaltyUnavailable);
            }
            let rows: Vec<AuditDay> =
                serde_json::from_value(body["result"].clone()).map_err(|_| LoyaltyUnavailable)?;
            if rows.len() >= self.config.loyalty_audit_row_limit {
                return Err(LoyaltyUnavailable);
            }
            for row in rows {
                days.entry(row.user_id).or_default().insert(row.signin_day);
            }
        }
        Ok(days)
    }
}
pub struct SupabaseLoyalty {
    mappings: PgPool,
    pub client: SupabaseClient,
}
impl SupabaseLoyalty {
    pub fn from_environment(config: &Config) -> Result<Self, LoyaltyUnavailable> {
        let raw = std::env::var("CORTEX_LOYALTY_DATABASE_URL").map_err(|_| LoyaltyUnavailable)?;
        let url = reqwest::Url::parse(&raw).map_err(|_| LoyaltyUnavailable)?;
        if url.path() != "/appdb" || url.username() != "appuser" {
            return Err(LoyaltyUnavailable);
        }
        let credentials = serde_json::from_str(
            &std::env::var("CORTEX_SUPABASE_CREDENTIALS").map_err(|_| LoyaltyUnavailable)?,
        )
        .map_err(|_| LoyaltyUnavailable)?;
        Ok(Self {
            mappings: PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(config.loyalty_source_timeout)
                .connect_lazy(&raw)
                .map_err(|_| LoyaltyUnavailable)?,
            client: SupabaseClient::new(
                config
                    .loyalty_api_base
                    .parse()
                    .map_err(|_| LoyaltyUnavailable)?,
                credentials,
                config.clone(),
            )?,
        })
    }
}
impl LoyaltySource for SupabaseLoyalty {
    fn fetch<'a>(
        &'a self,
        now: DateTime<Utc>,
    ) -> Pending<'a, Result<Vec<(String, LoyaltySnapshot)>, LoyaltyUnavailable>> {
        Box::pin(async move {
            let mut tx = self
                .mappings
                .begin()
                .await
                .map_err(|_| LoyaltyUnavailable)?;
            sqlx::query("SET TRANSACTION READ ONLY")
                .execute(&mut *tx)
                .await
                .map_err(|_| LoyaltyUnavailable)?;
            let mappings: Vec<(String,String)> = sqlx::query_as("SELECT uuid_id::text,supabase_user_id::text FROM public.defaultdb_user WHERE uuid_id IS NOT NULL AND supabase_user_id IS NOT NULL").fetch_all(&mut *tx).await.map_err(|_| LoyaltyUnavailable)?;
            tx.commit().await.map_err(|_| LoyaltyUnavailable)?;
            let ids = mappings
                .iter()
                .map(|(_, id)| Uuid::parse_str(id).map_err(|_| LoyaltyUnavailable))
                .collect::<Result<Vec<_>, _>>()?;
            let accounts = self.client.accounts(&ids, now).await?;
            let mut days = self.client.audit_days(&ids, now).await?;
            let mut snapshots = HashMap::new();
            for account in accounts {
                let days = days.entry(account.user_id).or_default();
                days.extend(account.session_days);
                if let Some(last) = account.last_sign_in_at
                    && last >= now - chrono::Duration::days(90)
                    && last <= now
                {
                    days.insert(last.date_naive());
                }
                snapshots.insert(
                    account.user_id,
                    LoyaltySnapshot::calculate(account.created_at, now, days.iter().copied()),
                );
            }
            Ok(mappings
                .into_iter()
                .filter_map(|(actor, id)| {
                    snapshots
                        .get(&Uuid::parse_str(&id).ok()?)
                        .map(|snapshot| (actor, *snapshot))
                })
                .collect())
        })
    }
}
