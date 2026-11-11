#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    use cortex_api::dal::local::database::{self, Target};
    let target = match std::env::args().nth(1).as_deref() {
        Some("test") => Target::Test,
        Some("cortex") => Target::Primary,
        _ => {
            eprintln!("Use migrate test or migrate cortex");
            std::process::exit(1)
        }
    };
    let pool = database::connect(target).await.unwrap_or_else(|message| {
        eprintln!("{message}");
        std::process::exit(1)
    });
    if sqlx::migrate!().run(&pool).await.is_err() {
        eprintln!("The database migrations failed");
        std::process::exit(1)
    }
    println!("Database migrations passed");
}
