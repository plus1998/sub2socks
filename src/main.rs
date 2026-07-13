use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use rust_proxy_manager::{
    config,
    db::Database,
    mihomo::{MihomoProcess, MihomoStatus},
    node_test::{self, NodeTestOutcome},
    subscriber,
    types::{InitRequest, ProxyNode, SocksAccount, Subscription},
};

#[derive(Clone)]
struct AppState {
    db: Database,
    mihomo: Arc<Mutex<MihomoProcess>>,
    sessions: Arc<Mutex<HashMap<String, Instant>>>,
    node_test_jobs: Arc<Mutex<HashMap<String, NodeTestJob>>>,
}

const SESSION_COOKIE: &str = "rpm_session";
const SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_RETAINED_NODE_TEST_JOBS: usize = 32;
const NODE_TEST_CONCURRENCY: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NodeTestJobStatus {
    Running,
    Completed,
}

#[derive(Debug, Clone, Serialize)]
struct NodeTestJob {
    job_id: String,
    status: NodeTestJobStatus,
    total: usize,
    done: usize,
    ok: usize,
    failed: usize,
    #[serde(skip)]
    created_at: Instant,
}

#[derive(Debug, Serialize)]
struct NodeTestResponse {
    node_id: i64,
    tested_at: i64,
    ok: bool,
    latency_ms: Option<i64>,
    error: Option<String>,
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
struct LoginRequest {
    admin_user: String,
    admin_pass: String,
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
        sessions: Arc::new(Mutex::new(HashMap::new())),
        node_test_jobs: Arc::new(Mutex::new(HashMap::new())),
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

    let protected = Router::new()
        .route(
            "/api/subscriptions",
            get(list_subscriptions).post(add_subscription),
        )
        .route("/api/subscriptions/:id/sync", post(sync_subscription))
        .route("/api/subscriptions/:id", delete(delete_subscription))
        .route("/api/nodes", get(list_nodes))
        .route("/api/nodes/test-all", post(test_all_nodes))
        .route("/api/nodes/:id/test", post(test_single_node))
        .route("/api/nodes/:id/enabled", put(set_node_enabled))
        .route("/api/nodes/:id", delete(delete_node))
        .route("/api/node-tests/:job_id", get(get_node_test_job))
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
        .route("/api/auth/logout", post(logout))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let app = Router::new()
        .route("/", get(index))
        .route("/assets/app.css", get(frontend_style))
        .route("/assets/app.js", get(frontend_script))
        .route("/api/status", get(status))
        .route("/api/init", post(init))
        .route("/api/auth/login", post(login))
        .merge(protected)
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

async fn frontend_style() -> impl IntoResponse {
    (
        [("content-type", "text/css; charset=utf-8")],
        include_str!("static/app.css"),
    )
}

async fn frontend_script() -> impl IntoResponse {
    (
        [("content-type", "text/javascript; charset=utf-8")],
        include_str!("static/app.js"),
    )
}

async fn status(State(state): State<AppState>, request: Request) -> ApiResult<serde_json::Value> {
    let initialized = state.db.is_initialized().map_err(ApiError::db)?;
    let token = session_token(&request).map(str::to_string);
    let authenticated = if initialized {
        let now = Instant::now();
        let mut sessions = state.sessions.lock().await;
        sessions.retain(|_, expires_at| *expires_at > now);
        token
            .as_deref()
            .and_then(|token| sessions.get(token))
            .is_some_and(|expires_at| *expires_at > now)
    } else {
        false
    };
    let mut mihomo = state.mihomo.lock().await;
    Ok(Json(json!({
        "initialized": initialized,
        "authenticated": authenticated,
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
) -> Result<Response, ApiError> {
    if state.db.is_initialized().map_err(ApiError::db)? {
        return Err(ApiError::conflict("instance is already initialized"));
    }
    if payload.admin_user.trim().is_empty() || payload.admin_pass.is_empty() {
        return Err(ApiError::bad_request(
            "admin_user and admin_pass are required",
        ));
    }
    state
        .db
        .initialize(payload.admin_user.trim(), &payload.admin_pass)
        .map_err(ApiError::db)?;
    authenticated_response(&state, "initialized").await
}

async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    if !state
        .db
        .verify_admin(payload.admin_user.trim(), &payload.admin_pass)
        .map_err(ApiError::db)?
    {
        return Err(ApiError::unauthorized("invalid username or password"));
    }
    authenticated_response(&state, "authenticated").await
}

async fn logout(State(state): State<AppState>, request: Request) -> Result<Response, ApiError> {
    if let Some(token) = session_token(&request) {
        state.sessions.lock().await.remove(token);
    }
    let mut response = Json(ApiMessage {
        status: "logged_out",
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("rpm_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    Ok(response)
}

async fn authenticated_response(
    state: &AppState,
    status: &'static str,
) -> Result<Response, ApiError> {
    let token: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();
    state
        .sessions
        .lock()
        .await
        .insert(token.clone(), Instant::now() + SESSION_TTL);
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    let mut response = Json(ApiMessage { status }).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    Ok(response)
}

async fn require_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(token) = session_token(&request) else {
        return Err(ApiError::unauthorized("authentication required"));
    };
    let now = Instant::now();
    let mut sessions = state.sessions.lock().await;
    sessions.retain(|_, expires_at| *expires_at > now);
    if !sessions
        .get(token)
        .is_some_and(|expires_at| *expires_at > now)
    {
        return Err(ApiError::unauthorized("session expired"));
    }
    drop(sessions);
    Ok(next.run(request).await)
}

fn session_token(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{SESSION_COOKIE}=")))
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

async fn test_single_node(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<NodeTestResponse> {
    let node = state
        .db
        .get_node(id)
        .map_err(ApiError::db)?
        .ok_or_else(|| ApiError::not_found("node not found"))?;
    let binary_path = node_test_binary(&state).await;
    let result = run_and_save_node_test(&state.db, binary_path, node).await?;
    Ok(Json(result))
}

async fn test_all_nodes(State(state): State<AppState>) -> ApiResult<NodeTestJob> {
    let nodes = state.db.list_nodes().map_err(ApiError::db)?;
    let job_id = random_token(24);
    let job = NodeTestJob {
        job_id: job_id.clone(),
        status: if nodes.is_empty() {
            NodeTestJobStatus::Completed
        } else {
            NodeTestJobStatus::Running
        },
        total: nodes.len(),
        done: 0,
        ok: 0,
        failed: 0,
        created_at: Instant::now(),
    };
    insert_node_test_job(&state, job.clone()).await?;

    if !nodes.is_empty() {
        let state_for_job = state.clone();
        tokio::spawn(async move {
            run_node_test_job(state_for_job, job_id, nodes).await;
        });
    }

    Ok(Json(job))
}

async fn get_node_test_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> ApiResult<NodeTestJob> {
    let jobs = state.node_test_jobs.lock().await;
    jobs.get(&job_id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::not_found("node test job not found"))
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

async fn node_test_binary(state: &AppState) -> Option<PathBuf> {
    state.mihomo.lock().await.binary_path()
}

async fn run_and_save_node_test(
    db: &Database,
    binary_path: Option<PathBuf>,
    node: ProxyNode,
) -> Result<NodeTestResponse, ApiError> {
    let node_id = node.id;
    let result = node_test::test_node(binary_path, &node).await;
    persist_node_test_result(db, node_id, result)
}

fn persist_node_test_result(
    db: &Database,
    node_id: i64,
    result: NodeTestOutcome,
) -> Result<NodeTestResponse, ApiError> {
    let tested_at = unix_timestamp();
    let saved = db
        .save_node_test_result(
            node_id,
            tested_at,
            result.ok,
            result.latency_ms,
            result.error.as_deref(),
        )
        .map_err(ApiError::db)?;
    if !saved {
        return Err(ApiError::not_found("node not found"));
    }
    Ok(NodeTestResponse {
        node_id,
        tested_at,
        ok: result.ok,
        latency_ms: result.latency_ms,
        error: result.error,
    })
}

async fn run_node_test_job(state: AppState, job_id: String, nodes: Vec<ProxyNode>) {
    let binary_path = node_test_binary(&state).await;
    let queue = Arc::new(Mutex::new(VecDeque::from(nodes)));
    let worker_count = NODE_TEST_CONCURRENCY.min(queue.lock().await.len());
    let mut workers = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let state = state.clone();
        let job_id = job_id.clone();
        let binary_path = binary_path.clone();
        let queue = queue.clone();
        workers.push(tokio::spawn(async move {
            loop {
                let Some(node) = queue.lock().await.pop_front() else {
                    break;
                };
                let ok = run_and_save_node_test(&state.db, binary_path.clone(), node)
                    .await
                    .is_ok_and(|result| result.ok);
                update_node_test_job(&state, &job_id, ok).await;
            }
        }));
    }

    for worker in workers {
        let _ = worker.await;
    }
}

async fn update_node_test_job(state: &AppState, job_id: &str, ok: bool) {
    let mut jobs = state.node_test_jobs.lock().await;
    if let Some(job) = jobs.get_mut(job_id) {
        job.done += 1;
        if ok {
            job.ok += 1;
        } else {
            job.failed += 1;
        }
        if job.done >= job.total {
            job.status = NodeTestJobStatus::Completed;
        }
    }
}

async fn insert_node_test_job(state: &AppState, job: NodeTestJob) -> Result<(), ApiError> {
    let mut jobs = state.node_test_jobs.lock().await;
    while jobs.len() >= MAX_RETAINED_NODE_TEST_JOBS {
        let removable = jobs
            .iter()
            .filter(|(_, existing)| existing.status == NodeTestJobStatus::Completed)
            .min_by_key(|(_, existing)| existing.created_at)
            .map(|(id, _)| id.clone());
        if let Some(id) = removable {
            jobs.remove(&id);
        } else {
            return Err(ApiError::conflict(
                "too many node test jobs are currently running",
            ));
        }
    }
    jobs.insert(job.job_id.clone(), job);
    Ok(())
}

fn random_token(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
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

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_proxy_manager::types::NodeType;

    fn test_state() -> AppState {
        AppState {
            db: Database::open(":memory:").unwrap(),
            mihomo: Arc::new(Mutex::new(MihomoProcess::with_binary(
                "/definitely/missing/mihomo",
            ))),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            node_test_jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn sample_node(subscription_id: i64) -> ProxyNode {
        ProxyNode {
            id: 0,
            subscription_id,
            name: "node".to_string(),
            raw: "name: node\ntype: http\nserver: 127.0.0.1\nport: 8080".to_string(),
            node_type: NodeType::Http,
            server: "127.0.0.1".to_string(),
            port: 8080,
            username: None,
            password: None,
            enabled: true,
            created_at: 0,
            last_tested_at: None,
            last_test_ok: None,
            last_test_latency_ms: None,
            last_test_error: None,
        }
    }

    fn add_node(state: &AppState) -> ProxyNode {
        let subscription_id = state
            .db
            .add_subscription("sub", "https://example.com/sub")
            .unwrap();
        state
            .db
            .replace_subscription_nodes(subscription_id, &[sample_node(subscription_id)])
            .unwrap();
        state.db.list_nodes().unwrap().remove(0)
    }

    #[tokio::test]
    async fn missing_binary_is_returned_and_persisted_as_failure() {
        let state = test_state();
        let node = add_node(&state);

        let Json(response) = test_single_node(State(state.clone()), Path(node.id))
            .await
            .unwrap();
        assert!(!response.ok);
        assert!(response
            .error
            .as_deref()
            .unwrap()
            .contains("binary unavailable"));
        let saved = state.db.get_node(node.id).unwrap().unwrap();
        assert_eq!(saved.last_test_ok, Some(false));
        assert!(saved.last_tested_at.is_some());
        assert!(saved
            .last_test_error
            .as_deref()
            .unwrap()
            .contains("binary unavailable"));
    }

    #[tokio::test]
    async fn missing_node_returns_not_found() {
        let state = test_state();
        let error = test_single_node(State(state), Path(404)).await.unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn batch_job_completes_and_counts_failures() {
        let state = test_state();
        add_node(&state);

        let Json(started) = test_all_nodes(State(state.clone())).await.unwrap();
        assert_eq!(started.total, 1);
        for _ in 0..50 {
            let job = state
                .node_test_jobs
                .lock()
                .await
                .get(&started.job_id)
                .cloned()
                .unwrap();
            if job.status == NodeTestJobStatus::Completed {
                assert_eq!(job.done, 1);
                assert_eq!(job.ok, 0);
                assert_eq!(job.failed, 1);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("job did not complete");
    }

    #[tokio::test]
    async fn job_retention_is_bounded() {
        let state = test_state();
        for index in 0..(MAX_RETAINED_NODE_TEST_JOBS + 5) {
            insert_node_test_job(
                &state,
                NodeTestJob {
                    job_id: format!("job-{index}"),
                    status: NodeTestJobStatus::Completed,
                    total: 0,
                    done: 0,
                    ok: 0,
                    failed: 0,
                    created_at: Instant::now(),
                },
            )
            .await
            .unwrap();
        }
        assert_eq!(
            state.node_test_jobs.lock().await.len(),
            MAX_RETAINED_NODE_TEST_JOBS
        );
    }
}
