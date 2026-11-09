use cortex_api::core::config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().init();
    let config = Config::default();
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!(address = %config.listen, "Development server started");
    axum::serve(listener, axum::Router::new()).await?;
    Ok(())
}
