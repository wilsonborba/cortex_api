use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::str::FromStr;
pub enum Target {
    Test,
    Primary,
}
pub fn options(target: Target) -> Result<PgConnectOptions, &'static str> {
    let (key, database) = match target {
        Target::Test => ("CORTEX_TEST_DATABASE_URL", "cortex_api_test"),
        Target::Primary => ("CORTEX_DATABASE_URL", "cortex_api"),
    };
    let url = std::env::var(key).map_err(|_| "The database secret is missing")?;
    let options =
        PgConnectOptions::from_str(&url).map_err(|_| "The database address is not valid")?;
    if options.get_database() != Some(database) || options.get_username() != "cortex_api_app" {
        return Err("The database name or role is not approved");
    }
    Ok(options)
}
pub async fn connect(target: Target) -> Result<PgPool, &'static str> {
    PgPoolOptions::new()
        .max_connections(crate::core::config::Config::default().database_connections)
        .connect_with(options(target)?)
        .await
        .map_err(|_| "The database connection failed")
}
