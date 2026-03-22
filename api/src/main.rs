mod config;
mod db;
mod firecracker;
mod mpp;
mod network;
mod routes;
mod vm;
mod x402;

#[cfg(test)]
mod tests;

use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use config::Config;
use db::Database;
use network::ip_pool::IpPool;
use vm::manager::VmManager;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub vm_manager: Arc<VmManager>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mpp_hosting_api=info,tower_http=info".into()),
        )
        .init();

    // Load configuration
    let config = Config::from_env()?;
    info!(
        listen = %config.listen_addr,
        subnet = %config.vm_subnet,
        "Starting MPP Hosting API"
    );

    // Initialize database
    let db = Arc::new(Database::open(&config.db_path)?);

    // Initialize IP pool with existing allocations
    let existing_ips = db.get_allocated_ips()?;
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &existing_ips)?);

    // Initialize VM manager
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool.clone()));

    let state = AppState {
        config: Arc::new(config.clone()),
        vm_manager: vm_manager.clone(),
    };

    // Build router
    let app = Router::new()
        // Health check (no payment required)
        .route("/health", get(routes::health::health))
        .route("/status", get(routes::health::status))
        .route("/.well-known/x402", get(routes::discovery::well_known_x402))
        .route("/openapi.json", get(routes::discovery::openapi))
        // Provision endpoint (MPP payment gated)
        // POST = actual provision, GET = always returns 402 (for x402 discovery probes)
        .route(
            "/v1/provision",
            post(routes::provision::provision)
                .get(routes::provision::provision_info)
                .route_layer(middleware::from_fn_with_state(
                    state.clone(),
                    mpp::middleware::mpp_payment_gate,
                )),
        )
        // V2 provision endpoint (MPP payment gated, wallet-auth SSH)
        .route(
            "/v2/provision",
            post(routes::v2::provision_v2).route_layer(middleware::from_fn_with_state(
                state.clone(),
                mpp::middleware::mpp_payment_gate,
            )),
        )
        // V2 session and auth endpoints (no payment gate)
        .route("/v2/session", post(routes::v2::create_session))
        .route("/v2/auth/verify", post(routes::v2::verify_auth))
        // Jobs endpoint (MPP payment gated)
        .route(
            "/v1/jobs",
            post(routes::jobs::create_job).route_layer(middleware::from_fn_with_state(
                state.clone(),
                mpp::middleware::mpp_payment_gate,
            )),
        )
        // Job status (no payment gate — authenticated by job_id knowledge)
        .route("/v1/jobs/{id}", get(routes::jobs::get_job))
        // VM management (no payment gate — authenticated by vm_id knowledge)
        .route("/v1/vms/{id}", get(routes::vm::get_vm))
        .route("/v1/vms/{id}", delete(routes::vm::delete_vm))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Spawn background cleanup task
    let cleanup_manager = vm_manager.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            cleanup_manager.cleanup_expired().await;
        }
    });

    // Start server
    let listener = tokio::net::TcpListener::bind(&config.listen_addr).await?;
    info!(addr = %config.listen_addr, "MPP Hosting API listening");
    axum::serve(listener, app).await?;

    Ok(())
}
