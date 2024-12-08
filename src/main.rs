use std::sync::Arc;
use sqlx::{
    migrate::{MigrateDatabase, Migrator},
    FromRow, Sqlite, SqlitePool
};
use axum::{
    response::{IntoResponse, Response},
    routing::{get, post},
    extract::{Path, State},
    http::{Method, header, StatusCode},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use chrono::{DateTime,Utc};
use mail_send::{SmtpClientBuilder, mail_builder::MessageBuilder};
use clap::Parser;
use anyhow::{anyhow, Result, Context};
use tower::{ServiceBuilder};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, error, instrument, debug};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

// Make our own error that wraps `anyhow::Error`.
struct AppError(anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{:?}", self.0),  // Format with {:#} in prod? IDK
        ).into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

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
struct AppContext {
    config: Arc<Config>,
    db: SqlitePool
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load Env File
    dotenv::dotenv().ok();
    // Init Tracing
    tracing_subscriber::fmt::init();
    // Parse Env/CLI
    let config = Config::parse();
    // Prepare DB
    // Create SQLite if it doesn't exist
    if !Sqlite::database_exists(&config.database_url).await? {
        info!("Missing DB - Creating database {}", &config.database_url);
        Sqlite::create_database(&config.database_url).await.context("Failed to create DB file.")?;
        info!("DB created");
    } else {
        info!("Database already exists");
    }
    // Open DB
    let db = SqlitePool::connect(&config.database_url).await?;
    // Migrate if needed
    MIGRATOR.run(&db).await.context("Migration failed!")?;
    info!("DB Migration completed");
    // TEST - Empty last_pub_date
    // sqlx::query("UPDATE rss_feeds SET last_pub_date = NULL").execute(&db).await.unwrap();
    // Prepare Web Server Context
    let context = AppContext {config: Arc::new(config), db: db.clone()};
    // Prepare Middlewares
    let cors = CorsLayer::new()
        // allow `GET` and `POST` when accessing the resource
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        // allow headers in requests
        .allow_headers([header::CONTENT_TYPE])
        // allow requests from any origin
        .allow_origin(Any);
    let middlewares = ServiceBuilder::new()
        .layer(cors);
    // Launch Web Server
    let app = Router::new()
        // `GET /` goes to `root`
        .route("/", get(root))
        .route("/feeds/:id/", get(get_feed).put(modify_feed).delete(delete_feed))
        .route("/feeds/:id/forcesend", post(force_send_feed))
        .route("/feeds/", get(get_feeds).post(create_feed))
        .layer(middlewares)
        .with_state(context.clone());  // TODO: AXUM LOG REQUESTS
    tokio::spawn(async move {
        send_feeds_scheduler(&context).await;
    });
    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.context("Failed to bind Web Service on 0.0.0.0:3000")?;
    info!("Listening on 0.0.0.0:3000");
    axum::serve(listener, app).await.context("Failed to serve Web Service")?;
    Ok(())
}

#[instrument(skip_all)]
async fn send_feeds_scheduler(ctx: &AppContext) {
    loop {
        if let Err(e) = send_feeds(ctx).await {
            error!("{}", e);
        }
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    }
}

async fn send_feeds(ctx: &AppContext) -> Result<()> {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&ctx.db).await?;
    for feed in feeds.into_iter() {
        info!("Spawn send_feed number {}", feed.id);
        let ctx2 = ctx.clone();
        tokio::spawn(async move {
            check_send_feed(&ctx2, feed).await;
        });
    }
    Ok(())
}

#[instrument(skip_all, fields(id = feed.id))]
async fn check_send_feed(ctx: &AppContext, feed: RssFeed) {
    // Here we can create a thiserror to choose action for various errors.
    let try_block = async move {
        let body = reqwest::get(feed.feed_url.clone()).await.context("RSS Fetch failed")?.bytes().await?;
        let channel = rss::Channel::read_from(&body[..]).context("RSS channel read failed")?;
        let item = channel.items.into_iter().next().ok_or(anyhow!("RSS channel item empty"))?;
        let pub_date = DateTime::parse_from_rfc2822(item.pub_date.as_ref()
            .ok_or(anyhow!("RSS Item missing pub_date"))?).context("Failed RFC2822 RSS pub_date parsing")?;
        let link = item.link.as_ref().ok_or(anyhow!("RSS Item missing link"))?;
        debug!("PubDate: {} and Link: {}", pub_date, link);
        match feed.last_pub_date {
            Some(last_pub_date) if last_pub_date == pub_date => {
                // Do nothing
                Result::<()>::Ok(())
            },
            _ => {
                send_notification(ctx, &feed, &item).await?;
                // Update the db
                sqlx::query("UPDATE rss_feeds SET last_pub_date = $1 WHERE id = $2")
                    .bind(pub_date).bind(feed.id)
                    .execute(&ctx.db).await.context("Failed to set last_pub_date in DB").map(|_| {})
            }
        }
    };
    if let Err(e) = try_block.await {
        error!("{:?}", e);
    }
}

async fn send_notification(ctx: &AppContext, feed: &RssFeed, rssitem: &rss::Item) -> Result<()> {
    // Build a simple multipart message
    let link = rssitem.link.as_ref().ok_or(anyhow!("RSS Item missing link"))?;
    info!("Sending Mail Notification for feed {}", feed.id);
    let title = rssitem.title.as_ref().ok_or(anyhow!("RSS Item missing title"))?;
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
        .connect().await.context("Error connecting to SMTP Server")?
        .send(message).await.context("Error sending message to SMTP Server")
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
    State(ctx): State<AppContext>,
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
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>,
    Json(payload): Json<CreateRssFeed>,
) -> (StatusCode, Json<RssFeed>) {
    sqlx::query("UPDATE rss_feeds SET name = $1, feed_url = $2 WHERE id = $3")
        .bind(&payload.name).bind(&payload.feed_url).bind(feed_id)
        .execute(&ctx.db).await.unwrap();
    get_feed(State(ctx), Path(feed_id)).await
}

async fn delete_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>
) -> StatusCode {
    sqlx::query("DELETE FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .execute(&ctx.db).await.unwrap();
    StatusCode::NO_CONTENT
}

async fn get_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>
) -> (StatusCode, Json<RssFeed>) {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&ctx.db).await.unwrap();
    (StatusCode::OK, Json(feed))
}

async fn get_feeds(
    State(ctx): State<AppContext>
) -> (StatusCode, Json<Vec<RssFeed>>) {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&ctx.db).await.unwrap();
    (StatusCode::OK, Json(feeds))
}

async fn force_send_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>
) -> StatusCode {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&ctx.db).await.unwrap();
    tokio::spawn(async move {
        check_send_feed(&ctx, feed).await;
    });
    StatusCode::OK
}
