use std::env;
use sqlx::{migrate::MigrateDatabase, FromRow, Sqlite, SqlitePool};
use axum::{
    routing::{get, post},
    extract::{Path, State},
    http::StatusCode,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use chrono::{DateTime,Utc};
use mail_send::{SmtpClientBuilder, mail_builder::MessageBuilder};

#[tokio::main]
async fn main() {
    // Init Tracing
    tracing_subscriber::fmt::init();
    // Prepare DB

    let db_url = env::var("DATABASE_URL").unwrap_or(String::from("sqlite://sqlite.db"));
    // Create SQLite if it doesn't exist
    if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
        println!("Missing DB - Creating database {}", &db_url);
        match Sqlite::create_database(&db_url).await {
            Ok(_) => println!("Create db success"),
            Err(error) => panic!("error: {}", error),
        }
    } else {
        println!("Database already exists");
    }
    // Open DB
    let db = SqlitePool::connect(&db_url).await.unwrap();
    // Migrate if needed
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let migrations = std::path::Path::new(&crate_dir).join("./migrations");
    let migration_results = sqlx::migrate::Migrator::new(migrations)
        .await
        .unwrap()
        .run(&db)
        .await;
    match migration_results {
        Ok(_) => println!("Migration success"),
        Err(error) => {
            panic!("Migration error: {}", error);
        }
    }
    // TEST - Empty last_pub_date
    sqlx::query("UPDATE rss_feeds SET last_pub_date = NULL").execute(&db).await.unwrap();
    // Arrange Web Server
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/feeds/:id/", get(get_feed).put(modify_feed).delete(delete_feed))
        .route("/feeds/:id/forcesend", post(force_send_feed))
        .route("/feeds/", get(get_feeds).post(create_feed))
        .with_state(db.clone());

    tokio::spawn(send_feeds_scheduler(db));
    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn send_feeds_scheduler(db: SqlitePool) {
    loop {
        send_feeds(db.clone()).await;
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    }
}

async fn send_feeds(db: SqlitePool) {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&db).await.unwrap();
    for feed in feeds.into_iter() {
        tracing::info!("Spawn send_feed number {}", feed.id);
        tokio::spawn(send_feed(db.clone(), feed));
    }
}

async fn send_feed(db: SqlitePool, feed: RssFeed) {
    let body = reqwest::get(feed.feed_url.clone()).await.unwrap().bytes().await.unwrap();
    let channel = rss::Channel::read_from(&body[..]).unwrap();
    let item = channel.items.into_iter().next().unwrap();
    let pub_date = DateTime::parse_from_rfc2822(item.pub_date.as_ref().unwrap()).unwrap();
    let link = item.link.as_ref().unwrap();
    tracing::info!("PubDate: {} and Link: {}", pub_date, link);
    match feed.last_pub_date {
        Some(last_pub_date) if last_pub_date == pub_date => {
            // Do nothing
        },
        _ => {
            let smtp_settings = SmtpSettings::retrieve(&db).await;
            send_notification(smtp_settings, &feed, &item).await;
            // Update the db
            sqlx::query("UPDATE rss_feeds SET last_pub_date = $1 WHERE id = $2")
                .bind(pub_date).bind(feed.id)
                .execute(&db).await.unwrap();
        }
    }
}

async fn send_notification(ss: SmtpSettings, feed: &RssFeed, rssitem: &rss::Item) {
    // Build a simple multipart message
    let link = rssitem.link.as_ref().unwrap();
    tracing::info!("SENDING NOTIFICATION WITH LINK: {}", link);
    let title = rssitem.title.as_ref().unwrap();
    let description = rssitem.description.as_ref().map_or("", |x| x.as_str());
    let from_name = format!("RSS {}", feed.name);
    let html_body = format!("<p>Original Post: <a href=\"{}\">{}</a></p>{}", link, title, description);
    let text_body = format!("Original Post: {} - {}\r\n", title, link);
    let message = MessageBuilder::new()
        .from((from_name.as_str(), ss.from_email.as_str()))
        .to(ss.to_email.as_str())
        .subject(title)
        .html_body(html_body)
        .text_body(text_body);
    SmtpClientBuilder::new(ss.host, ss.port)
        .implicit_tls(false)
        .credentials((ss.auth_user, ss.auth_password))
        .connect().await.unwrap()
        .send(message).await.unwrap();
}

async fn root() -> &'static str{
    "Hello World!\n"
}

#[derive(Deserialize)]
struct CreateRssFeed {
    name: String,
    feed_url: String,
}

#[derive(Serialize,FromRow)]
struct RssFeed {
    id: u32,
    name: String,
    feed_url: String,
    last_pub_date: Option<DateTime<Utc>>,
}

async fn create_feed(
    State(db): State<SqlitePool>,
    Json(payload): Json<CreateRssFeed>,
) -> (StatusCode, Json<RssFeed>) {
    let mut transaction = db.begin().await.unwrap();
    sqlx::query("INSERT INTO rss_feeds (name, feed_url)\
    VALUES ($1, $2)")
        .bind(&payload.name).bind(&payload.feed_url)
        .execute(&mut *transaction).await.unwrap();
    let (id,): (u32,) = sqlx::query_as("SELECT last_insert_rowid()").fetch_one(&mut *transaction).await.unwrap();
    transaction.commit().await.unwrap();
    let result = RssFeed{
        id,
        name:payload.name,
        feed_url:payload.feed_url,
        last_pub_date:None,
    };
    (StatusCode::CREATED, Json(result))
}

async fn modify_feed(
    State(db): State<SqlitePool>,
    Path(feed_id): Path<u32>,
    Json(payload): Json<CreateRssFeed>,
) -> (StatusCode, Json<RssFeed>) {
    sqlx::query("UPDATE rss_feeds SET name = $1, feed_url = $2 WHERE id = $3")
        .bind(&payload.name).bind(&payload.feed_url).bind(feed_id)
        .execute(&db).await.unwrap();
    get_feed(State(db), Path(feed_id)).await
}

async fn delete_feed(
    State(db): State<SqlitePool>,
    Path(feed_id): Path<u32>
) -> StatusCode {
    sqlx::query("DELETE FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .execute(&db).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn get_feed(
    State(db): State<SqlitePool>,
    Path(feed_id): Path<u32>
) -> (StatusCode, Json<RssFeed>) {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&db).await.unwrap();
    (StatusCode::OK, Json(feed))
}

async fn get_feeds(
    State(db): State<SqlitePool>
) -> (StatusCode, Json<Vec<RssFeed>>) {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&db).await.unwrap();
    (StatusCode::OK, Json(feeds))
}

async fn force_send_feed(
    State(db): State<SqlitePool>,
    Path(feed_id): Path<u32>
) -> StatusCode {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&db).await.unwrap();
    tokio::spawn(send_feed(db.clone(), feed));
    StatusCode::OK
}

#[derive(FromRow)]
struct SmtpSettings {
    host: String,
    port: u16,
    from_email: String,
    from_name: String,
    to_email: String,
    auth_user: String,
    auth_password: String,
}

impl SmtpSettings {
    async fn retrieve(db: &SqlitePool) -> SmtpSettings {
        sqlx::query_as(
            "SELECT host, port, from_email, from_name, to_email, auth_user, auth_password FROM smtp_settings WHERE id = 1"
        ).fetch_one(db).await.unwrap()
    }
}

