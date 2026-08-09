fn main() {
    dotenvy::dotenv().ok();
    println!("WEB_SEARCH_CMD: {:?}", std::env::var("WEB_SEARCH_CMD"));
    println!("DATABASE_URL: {:?}", std::env::var("DATABASE_URL"));
}
