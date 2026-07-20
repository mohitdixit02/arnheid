use anyhow::Result;
use arnheid::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    println!("Config loaded successfully: {:?}", config.embedding_model);
    Ok(())
}
