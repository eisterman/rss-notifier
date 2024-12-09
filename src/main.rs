use anyhow::{anyhow, Result, Context};
use axum::{
    extract::{Path, State},
    http::{Method, header, StatusCode, Request, Uri},
    response::{IntoResponse, Response, Html},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime,Utc};
use clap::Parser;
use mail_send::{SmtpClientBuilder, mail_builder::MessageBuilder};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use sqlx::{
    migrate::{MigrateDatabase, Migrator},
    FromRow, Sqlite, SqlitePool
};
use std::sync::Arc;
use tower::{ServiceBuilder};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{TraceLayer, DefaultOnResponse},
    LatencyUnit
};
use tracing::{
    info, error, debug, info_span, enabled,
    instrument, Level, Span
};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

// Make our own error that wraps `anyhow::Error`.
struct AppError(anyhow::Error);
// TODO: use thiserror to granularize the errors and differentiate the return response.
//  for example I don't want the SQLErrors to be sent directly if not in Debug Mode, but other more
//  simple errors like "obj not found" need to be sent as-they-are.

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = if enabled!(Level::DEBUG) {
            format!("{:?}", self.0)  // Format with verbose {:?} with Debug enabled
        } else {
            format!("{:#}", self.0)
        };
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
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
    http_host: String,
    #[arg(long,env)]
    http_port: u16,
    #[arg(long,env)]
    polling_time_sec: u64,
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
    let tracelayer = TraceLayer::new_for_http()
        .make_span_with(|request: &Request<_>| {
            // Log the matched route's path (with placeholders not filled in).
            let path = request.uri().to_string();
            info_span!(
                "request",
                method = ?request.method(),
                path
            )
        })
        .on_request(|_request: &Request<_>, _span: &Span| {
            debug!("Request received");
        })
        .on_response(
            DefaultOnResponse::new()
                .level(Level::DEBUG)
                .latency_unit(LatencyUnit::Millis)
        );
    let middlewares = ServiceBuilder::new()
        .layer(tracelayer).layer(cors);
    // Launch Web Server
    let app = Router::new()
        .route("/feeds/:id/", get(get_feed).put(modify_feed).delete(delete_feed))
        .route("/feeds/:id/forcesend", post(force_send_feed))
        .route("/feeds/", get(get_feeds).post(create_feed))
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route("/*file", get(static_handler))
        .layer(middlewares)
        .with_state(context.clone());  // TODO: AXUM LOG REQUESTS
    let context2 = context.clone();
    tokio::spawn(async move {
        send_feeds_scheduler(&context2).await;
    });
    // run our app with hyper, listening globally on port 3000
    let listener = tokio::net::TcpListener::bind((context.config.http_host.as_str(), context.config.http_port))
        .await.context(format!("Failed to bind Web Service on {}:{}", context.config.http_host, context.config.http_port))?;
    info!("Listening on {}:{}", context.config.http_host, context.config.http_port);
    axum::serve(listener, app).await.context("Failed to serve Web Service")?;
    Ok(())
}

#[instrument(skip_all)]
async fn send_feeds_scheduler(ctx: &AppContext) {
    loop {
        if let Err(e) = send_feeds(ctx).await {
            error!("{}", e);
        }
        tokio::time::sleep(std::time::Duration::from_secs(ctx.config.polling_time_sec)).await;
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
        if enabled!(Level::DEBUG) {
            error!("{:?}", e); // Format with verbose {:?} with Debug enabled
        } else {
            error!("{:#}", e);
        };
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

// TODO: Better errors for all the endpoints
async fn create_feed(
    State(ctx): State<AppContext>,
    Json(payload): Json<CreateRssFeed>,
) -> Result<(StatusCode, Json<RssFeed>), AppError> {
    let mut transaction = ctx.db.begin().await?;
    sqlx::query("INSERT INTO rss_feeds (name, feed_url)\
    VALUES ($1, $2)")
        .bind(&payload.name).bind(&payload.feed_url)
        .execute(&mut *transaction).await?;
    let (id,): (u32,) = sqlx::query_as("SELECT last_insert_rowid()").fetch_one(&mut *transaction).await?;
    transaction.commit().await?;
    let result = RssFeed{
        id,
        name:payload.name,
        feed_url:payload.feed_url,
        last_pub_date:None,
    };
    Ok((StatusCode::CREATED, Json(result)))
}

async fn modify_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>,
    Json(payload): Json<CreateRssFeed>,
) -> Result<(StatusCode, Json<RssFeed>), AppError> {
    sqlx::query("UPDATE rss_feeds SET name = $1, feed_url = $2 WHERE id = $3")
        .bind(&payload.name).bind(&payload.feed_url).bind(feed_id)
        .execute(&ctx.db).await?;
    get_feed(State(ctx), Path(feed_id)).await
}

async fn delete_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>
) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .execute(&ctx.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>
) -> Result<(StatusCode, Json<RssFeed>), AppError> {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&ctx.db).await?;
    Ok((StatusCode::OK, Json(feed)))
}

async fn get_feeds(
    State(ctx): State<AppContext>
) -> Result<(StatusCode, Json<Vec<RssFeed>>), AppError> {
    let feeds: Vec<RssFeed> = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds ORDER BY id")
        .fetch_all(&ctx.db).await.context("Query Error")?;
    Ok((StatusCode::OK, Json(feeds)))
}

async fn force_send_feed(
    State(ctx): State<AppContext>,
    Path(feed_id): Path<u32>
) -> Result<StatusCode, AppError> {
    let feed: RssFeed = sqlx::query_as("SELECT id, name, feed_url, last_pub_date FROM rss_feeds WHERE id = $1")
        .bind(feed_id)
        .fetch_one(&ctx.db).await?;
    tokio::spawn(async move {
        check_send_feed(&ctx, feed).await;
    });
    Ok(StatusCode::OK)
}

// Fallback Route
fn not_found_body() -> Html<&'static str> {
    Html("<h1>404</h1><p>Not Found</p>")
}

// Static Handlers
async fn index_handler() -> impl IntoResponse {
    static_handler("/index.html".parse::<Uri>().unwrap()).await
}

// Handler that takes the data from the Embed Asset storage
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/').to_string();
    StaticFile(path)
}

#[derive(Embed)]
#[folder = "frontend/build/"]
struct Asset;

// This wrapper is for allowind impl IntoResponse
pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();

        match Asset::get(path.as_str()) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
            }
            None => (StatusCode::NOT_FOUND, not_found_body()).into_response(),
        }
    }
}
