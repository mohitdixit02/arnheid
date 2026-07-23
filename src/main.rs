use anyhow::Result;
use arnheid::config::Config;
use arnheid::db;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = db::init_pool(&config.database_url).await?;
    println!("Database connected!");
    Ok(())
}
