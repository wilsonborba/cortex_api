use cortex_api::core::config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().init();
    let config = Config::default();
    use cortex_api::dal::remote::runs::{RunDatabase, RunStore};
    match RunStore::from_environment(RunDatabase::Primary) {
        Ok(store) => {
            if store.initialize().await.is_err() {
                tracing::warn!("Run record database setup failed");
            }
        }
        Err(_) => tracing::warn!("Run record database is not configured"),
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %config.listen, "Development server started");
    axum::serve(
        listener,
        cortex_api::presentation::with_response_contract(axum::Router::new()),
    )
    .await?;
    Ok(())
}
