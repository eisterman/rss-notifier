use std::env;
use std::sync::Arc;
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
use clap::Parser;

#[derive(Parser)]
struct Config {
    #[arg(long,env)]
    database_url: String,
    #[arg(long,env)]
    smtp_host: String,
    #[arg(long,env)]
    smtp_port: u16,
    #[arg(long,env)]
    from_email: String,
    #[arg(long,env)]
    to_email: String,
    #[arg(long,env)]
    smtp_auth_user: String,
    #[arg(long,env)]
    smtp_auth_password: String,
}

#[derive(Clone)]
struct Context {
    config: Arc<Config>,
    db: SqlitePool
}

#[tokio::main]
async fn main() {
    // Load Env File
    dotenv::dotenv().ok();
    // Init Tracing
    tracing_subscriber::fmt::init();
    // Parse Env/CLI
    let config = Config::parse();
    // Prepare DB
    // Create SQLite if it doesn't exist
    if !Sqlite::database_exists(&config.database_url).await.unwrap_or(false) {
        println!("Missing DB - Creating database {}", &config.database_url);
        match Sqlite::create_database(&config.database_url).await {
            Ok(_) => println!("Create db success"),
            Err(error) => panic!("error: {}", error),
        }
    } else {
        println!("Database already exists");
    }
    // Open DB
    let db = SqlitePool::connect(&config.database_url).await.unwrap();
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
    let context = Context{config: Arc::new(config), db: db.clone()};
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/feeds/:id/", get(get_feed).put(modify_feed).delete(delete_feed))
        .route("/feeds/:id/forcesend", post(force_send_feed))
        .route("/feeds/", get(get_feeds).post(create_feed))
        .with_state(context.clone());
    tokio::spawn(async move {
        send_feeds_scheduler(&context).await;
    });
    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn send_feeds_scheduler(ctx: &Context) {
    loop {
        send_feeds(ctx).await;
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    }
}

async fn send_feeds(ctx: &Context) {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&ctx.db).await.unwrap();
    for feed in feeds.into_iter() {
        tracing::info!("Spawn send_feed number {}", feed.id);
        let ctx2 = ctx.clone();
        tokio::spawn(async move {
            send_feed(&ctx2, feed).await;
        });
    }
}

async fn send_feed(ctx: &Context, feed: RssFeed) {
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
            send_notification(ctx, &feed, &item).await;
            // Update the db
            sqlx::query("UPDATE rss_feeds SET last_pub_date = $1 WHERE id = $2")
                .bind(pub_date).bind(feed.id)
                .execute(&ctx.db).await.unwrap();
        }
    }
}

async fn send_notification(ctx: &Context, feed: &RssFeed, rssitem: &rss::Item) {
    // Build a simple multipart message
    let link = rssitem.link.as_ref().unwrap();
    tracing::info!("SENDING NOTIFICATION WITH LINK: {}", link);
    let title = rssitem.title.as_ref().unwrap();
    let description = rssitem.description.as_ref().map_or("", |x| x.as_str());
    let from_name = format!("RSS {}", feed.name);
    let html_body = format!("<p>Original Post: <a href=\"{}\">{}</a></p>{}", link, title, description);
    let text_body = format!("Original Post: {} - {}\r\n", title, link);
    let message = MessageBuilder::new()
        .from((from_name.as_str(), ctx.config.from_email.as_str()))
        .to(ctx.config.to_email.as_str())
        .subject(title)
        .html_body(html_body)
        .text_body(text_body);
    SmtpClientBuilder::new(ctx.config.smtp_host.as_str(), ctx.config.smtp_port)
        .implicit_tls(false)
        .credentials((ctx.config.smtp_auth_user.as_str(), ctx.config.smtp_auth_password.as_str()))
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
    State(ctx): State<Context>,
    Json(payload): Json<CreateRssFeed>,
) -> (StatusCode, Json<RssFeed>) {
    let mut transaction = ctx.db.begin().await.unwrap();
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
    State(ctx): State<Context>,
    Path(feed_id): Path<u32>,
    Json(payload): Json<CreateRssFeed>,
) -> (StatusCode, Json<RssFeed>) {
    sqlx::query("UPDATE rss_feeds SET name = $1, feed_url = $2 WHERE id = $3")
        .bind(&payload.name).bind(&payload.feed_url).bind(feed_id)
        .execute(&ctx.db).await.unwrap();
    get_feed(State(ctx), Path(feed_id)).await
}

async fn delete_feed(
    State(ctx): State<Context>,
    Path(feed_id): Path<u32>
) -> StatusCode {
    sqlx::query("DELETE FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .execute(&ctx.db).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn get_feed(
    State(ctx): State<Context>,
    Path(feed_id): Path<u32>
) -> (StatusCode, Json<RssFeed>) {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&ctx.db).await.unwrap();
    (StatusCode::OK, Json(feed))
}

async fn get_feeds(
    State(ctx): State<Context>
) -> (StatusCode, Json<Vec<RssFeed>>) {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&ctx.db).await.unwrap();
    (StatusCode::OK, Json(feeds))
}

async fn force_send_feed(
    State(ctx): State<Context>,
    Path(feed_id): Path<u32>
) -> StatusCode {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&ctx.db).await.unwrap();
    tokio::spawn(async move {
        send_feed(&ctx, feed).await;
    });
    StatusCode::OK
}
