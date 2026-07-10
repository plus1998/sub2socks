use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

use rust_proxy_manager::{
    config,
    db::Database,
    mihomo::{MihomoProcess, MihomoStatus},
    subscriber,
    types::{InitRequest, SocksAccount, Subscription},
};

#[derive(Clone)]
struct AppState {
    db: Database,
    mihomo: Arc<Mutex<MihomoProcess>>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct SubscriptionRequest {
    name: Option<String>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct EnabledRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct SocksAccountRequest {
    name: String,
    username: String,
    password: String,
    node_id: i64,
    enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ConfigPreview {
    yaml: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

type ApiResult<T> = Result<Json<T>, ApiError>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = AppState {
        db: Database::init()?,
        mihomo: Arc::new(Mutex::new(MihomoProcess::new())),
    };

    // Spawn SOCKS5 multiplexer (clone db before router consumes state)
    let socks_port = std::env::var("SOCKS_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(9999);
    let socks_addr = SocketAddr::from(([0, 0, 0, 0], socks_port));
    let socks_db = state.db.clone();
    tokio::spawn(async move {
        rust_proxy_manager::socks_proxy::serve(socks_addr, socks_db).await;
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/static/css/style.css", get(style))
        .route("/api/status", get(status))
        .route("/api/init", post(init))
        .route(
            "/api/subscriptions",
            get(list_subscriptions).post(add_subscription),
        )
        .route("/api/subscriptions/:id/sync", post(sync_subscription))
        .route("/api/subscriptions/:id", delete(delete_subscription))
        .route("/api/nodes", get(list_nodes))
        .route("/api/nodes/:id/enabled", put(set_node_enabled))
        .route("/api/nodes/:id", delete(delete_node))
        .route(
            "/api/socks-accounts",
            get(list_socks_accounts).post(add_socks_account),
        )
        .route(
            "/api/socks-accounts/:id",
            put(update_socks_account).delete(delete_socks_account),
        )
        .route(
            "/api/socks-accounts/:id/enabled",
            put(set_socks_account_enabled),
        )
        .route("/api/config/preview", get(config_preview))
        .route("/api/mihomo/status", get(mihomo_status))
        .route("/api/mihomo/start", post(mihomo_start))
        .route("/api/mihomo/stop", post(mihomo_stop))
        .with_state(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    println!("Rust Proxy Manager listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("static/index.html"))
}

async fn style() -> impl IntoResponse {
    (
        [("content-type", "text/css; charset=utf-8")],
        include_str!("static/css/style.css"),
    )
}

async fn status(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let mut mihomo = state.mihomo.lock().await;
    Ok(Json(json!({
        "initialized": state.db.is_initialized().map_err(ApiError::db)?,
        "mihomo": mihomo.status(),
        "socks_port": std::env::var("SOCKS_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(9999u16),
    })))
}

async fn init(
    State(state): State<AppState>,
    Json(payload): Json<InitRequest>,
) -> ApiResult<ApiMessage> {
    if payload.admin_user.trim().is_empty() || payload.admin_pass.is_empty() {
        return Err(ApiError::bad_request(
            "admin_user and admin_pass are required",
        ));
    }
    state
        .db
        .initialize(&payload.admin_user, &payload.admin_pass)
        .map_err(ApiError::db)?;
    Ok(Json(ApiMessage {
        status: "initialized",
    }))
}

async fn list_subscriptions(State(state): State<AppState>) -> ApiResult<Vec<Subscription>> {
    Ok(Json(state.db.list_subscriptions().map_err(ApiError::db)?))
}

async fn add_subscription(
    State(state): State<AppState>,
    Json(payload): Json<SubscriptionRequest>,
) -> ApiResult<serde_json::Value> {
    if payload.url.trim().is_empty() {
        return Err(ApiError::bad_request("subscription url is required"));
    }

    let name = payload
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| payload.url.clone());
    let id = state
        .db
        .add_subscription(&name, &payload.url)
        .map_err(ApiError::db)?;
    let subscription = state
        .db
        .get_subscription(id)
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::not_found("subscription was not created"))?;
    let count = sync_subscription_record(&state.db, &subscription).await?;

    Ok(Json(json!({ "id": id, "synced_nodes": count })))
}

async fn sync_subscription(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    let subscription = state
        .db
        .get_subscription(id)
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::not_found("subscription not found"))?;
    let count = sync_subscription_record(&state.db, &subscription).await?;
    Ok(Json(json!({ "id": id, "synced_nodes": count })))
}

async fn delete_subscription(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<ApiMessage> {
    state.db.delete_subscription(id).map_err(ApiError::db)?;
    Ok(Json(ApiMessage { status: "deleted" }))
}

async fn list_nodes(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    Ok(Json(json!({
        "nodes": state.db.list_nodes().map_err(ApiError::db)?,
    })))
}

async fn set_node_enabled(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<EnabledRequest>,
) -> ApiResult<ApiMessage> {
    state
        .db
        .set_node_enabled(id, payload.enabled)
        .map_err(ApiError::db)?;
    Ok(Json(ApiMessage { status: "updated" }))
}

async fn delete_node(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<ApiMessage> {
    state.db.delete_node(id).map_err(ApiError::db)?;
    Ok(Json(ApiMessage { status: "deleted" }))
}

async fn list_socks_accounts(State(state): State<AppState>) -> ApiResult<Vec<SocksAccount>> {
    Ok(Json(state.db.list_socks_accounts().map_err(ApiError::db)?))
}

async fn add_socks_account(
    State(state): State<AppState>,
    Json(payload): Json<SocksAccountRequest>,
) -> ApiResult<serde_json::Value> {
    validate_socks_account(&state.db, &payload, None)?;
    if state
        .db
        .get_node(payload.node_id)
        .map_err(ApiError::db)?
        .is_none()
    {
        return Err(ApiError::bad_request("target node does not exist"));
    }

    let id = state
        .db
        .add_socks_account(
            &payload.name,
            &payload.username,
            &payload.password,
            payload.node_id,
        )
        .map_err(ApiError::db)?;
    if payload.enabled == Some(false) {
        state
            .db
            .set_socks_account_enabled(id, false)
            .map_err(ApiError::db)?;
    }
    Ok(Json(json!({ "id": id })))
}

async fn update_socks_account(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<SocksAccountRequest>,
) -> ApiResult<ApiMessage> {
    validate_socks_account(&state.db, &payload, Some(id))?;
    if state
        .db
        .get_socks_account(id)
        .map_err(ApiError::db)?
        .is_none()
    {
        return Err(ApiError::not_found("socks account not found"));
    }
    if state
        .db
        .get_node(payload.node_id)
        .map_err(ApiError::db)?
        .is_none()
    {
        return Err(ApiError::bad_request("target node does not exist"));
    }

    let updated = state
        .db
        .update_socks_account(
            id,
            &payload.name,
            &payload.username,
            &payload.password,
            payload.node_id,
        )
        .map_err(ApiError::db)?;
    if !updated {
        return Err(ApiError::not_found("socks account not found"));
    }
    if let Some(enabled) = payload.enabled {
        state
            .db
            .set_socks_account_enabled(id, enabled)
            .map_err(ApiError::db)?;
    }
    Ok(Json(ApiMessage { status: "updated" }))
}

async fn delete_socks_account(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<ApiMessage> {
    state.db.delete_socks_account(id).map_err(ApiError::db)?;
    Ok(Json(ApiMessage { status: "deleted" }))
}

async fn set_socks_account_enabled(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<EnabledRequest>,
) -> ApiResult<ApiMessage> {
    state
        .db
        .set_socks_account_enabled(id, payload.enabled)
        .map_err(ApiError::db)?;
    Ok(Json(ApiMessage { status: "updated" }))
}

async fn config_preview(State(state): State<AppState>) -> ApiResult<ConfigPreview> {
    Ok(Json(ConfigPreview {
        yaml: build_config(&state.db)?,
    }))
}

async fn mihomo_status(State(state): State<AppState>) -> ApiResult<MihomoStatus> {
    let mut mihomo = state.mihomo.lock().await;
    Ok(Json(mihomo.status()))
}

async fn mihomo_start(State(state): State<AppState>) -> ApiResult<ApiMessage> {
    let yaml = build_config(&state.db)?;
    let mut mihomo = state.mihomo.lock().await;
    mihomo.start(&yaml).await.map_err(ApiError::internal)?;
    Ok(Json(ApiMessage { status: "started" }))
}

async fn mihomo_stop(State(state): State<AppState>) -> ApiResult<ApiMessage> {
    let mut mihomo = state.mihomo.lock().await;
    mihomo.stop().await.map_err(ApiError::internal)?;
    Ok(Json(ApiMessage { status: "stopped" }))
}

async fn sync_subscription_record(
    db: &Database,
    subscription: &Subscription,
) -> Result<usize, ApiError> {
    let parsed = subscriber::fetch_subscription(&subscription.url, subscription.id)
        .await
        .map_err(ApiError::bad_request)?;
    let count = parsed.nodes.len();
    if count == 0 {
        return Err(ApiError::bad_request(
            "subscription did not contain any supported nodes; existing nodes were not changed",
        ));
    }
    db.replace_subscription_nodes(subscription.id, &parsed.nodes)
        .map_err(ApiError::db)?;
    db.mark_subscription_synced(subscription.id)
        .map_err(ApiError::db)?;
    Ok(count)
}

fn build_config(db: &Database) -> Result<String, ApiError> {
    let nodes = db.list_enabled_nodes().map_err(ApiError::db)?;
    let accounts = db.list_enabled_socks_accounts().map_err(ApiError::db)?;
    config::generate_mihomo_config(&nodes, &accounts).map_err(ApiError::internal)
}

fn validate_socks_account(
    db: &Database,
    payload: &SocksAccountRequest,
    exclude_id: Option<i64>,
) -> Result<(), ApiError> {
    if payload.name.trim().is_empty()
        || payload.username.trim().is_empty()
        || payload.password.is_empty()
    {
        return Err(ApiError::bad_request(
            "name, username, and password are required",
        ));
    }
    // Check username uniqueness
    if let Some(existing) = db
        .find_account_by_username(&payload.username)
        .map_err(ApiError::db)?
    {
        if Some(existing.id) != exclude_id {
            return Err(ApiError::bad_request("username already in use"));
        }
    }
    Ok(())
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn db(error: rusqlite::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }

    fn internal(error: impl ToString) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.message,
            })),
        )
            .into_response()
    }
}
