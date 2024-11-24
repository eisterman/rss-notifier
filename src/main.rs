use sqlx::{migrate::MigrateDatabase, FromRow, Row, Sqlite, SqlitePool};
const DB_URL: &str = "sqlite://sqlite.db";

#[tokio::main]
async fn main() {
    // Create SQLite if it doesn't exist
    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        println!("Missing DB - Creating database {}", DB_URL);
        match Sqlite::create_database(DB_URL).await {
            Ok(_) => println!("Create db success"),
            Err(error) => panic!("error: {}", error),
        }
    } else {
        println!("Database already exists");
    }
    // Open DB
    let db = SqlitePool::connect(DB_URL).await.unwrap();
    // Migrate if needed
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let migrations = std::path::Path::new(&crate_dir).join("./migrations");
    let migration_results = sqlx::migrate::Migrator::new(migrations)
        .await
        .unwrap()
        .run(&db)
        .await;
    match migration_results {
        Ok(_) => println!("Migration success"),
        Err(error) => {
            panic!("error: {}", error);
        }
    }
    println!("migration: {:?}", migration_results);
    //
}
