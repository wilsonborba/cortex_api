//! File worker binary (7.10.4): install it on another device and set the job
//! location to `Remote(url)` in config.rs to move extract, ocr or transcribe.
use cortex_api::core::config::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::default();
    let listener = tokio::net::TcpListener::bind(config.worker_listen).await?;
    eprintln!("File worker started on {}", config.worker_listen);
    axum::serve(
        listener,
        cortex_api::presentation::with_response_contract(
            cortex_api::presentation::file_worker::router(config),
        ),
    )
    .await?;
    Ok(())
}
