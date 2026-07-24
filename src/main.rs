use anyhow::Result;
use arnheid::config::Config;
use arnheid::{db, bot};
use teloxide::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let pool = db::init_pool(&config.database_url).await?;
    let bot = Bot::new(&config.telegram_bot_token);
    println!("Bot initialized.");
    Ok(())
}
