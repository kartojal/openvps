use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use base64::Engine;
use std::sync::Arc;
use tower::ServiceExt;

use crate::config::Config;
use crate::db::Database;
use crate::network::ip_pool::IpPool;
use crate::vm::manager::VmManager;
use crate::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a test Config with dev mode enabled and safe defaults.
fn test_config() -> Config {
    Config {
        listen_addr: "127.0.0.1:0".parse().unwrap(),
        firecracker_bin: "/nonexistent/firecracker".to_string(),
        vm_state_dir: "/tmp/mpp-test-vms".to_string(),
        kernel_path: "/nonexistent/vmlinux".to_string(),
        rootfs_path: "/nonexistent/rootfs.ext4".to_string(),
        vm_subnet: "172.16.0.0/24".to_string(),
        bridge_iface: "test-br0".to_string(),
        db_path: ":memory:".to_string(),
        mpp_secret_key: "test-secret-key-for-tests".to_string(),
        mpp_dev_mode: true,
        tempo_rpc_url: "https://rpc.tempo.xyz".to_string(),
        payment_recipient: "0x1234567890abcdef1234567890abcdef12345678".to_string(),
        usdc_contract: "0x20c000000000000000000000b9537d11c60e8b50".to_string(),
        chain_id: 4217,
        price_vcpu_hour_micro: 5000,
        price_ram_mb_hour_micro: 2,
        price_disk_gb_hour_micro: 100,
        public_ip: "0.0.0.0".to_string(),
        host_iface: "eth0".to_string(),
        ssh_port_base: 2200,
        x402_facilitator_url: "https://x402.org/facilitator".to_string(),
        x402_base_asset: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".to_string(),
        x402_celo_asset: "0xcebA9300f2b948710d2653dD7B07f33A8B32118C".to_string(),
    }
}

/// Build the full app Router identical to main.rs, but with an in-memory DB.
fn test_app() -> Router {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().expect("in-memory DB should open"));
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).expect("IP pool should init"));
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));

    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    Router::new()
        .route("/health", get(crate::routes::health::health))
        .route("/status", get(crate::routes::health::status))
        .route(
            "/.well-known/x402",
            get(crate::routes::discovery::well_known_x402),
        )
        .route("/openapi.json", get(crate::routes::discovery::openapi))
        .route(
            "/v1/provision",
            post(crate::routes::provision::provision)
                .get(crate::routes::provision::provision_info)
                .route_layer(middleware::from_fn_with_state(
                    state.clone(),
                    crate::mpp::middleware::mpp_payment_gate,
                )),
        )
        .route(
            "/v2/provision",
            post(crate::routes::v2::provision_v2).route_layer(middleware::from_fn_with_state(
                state.clone(),
                crate::mpp::middleware::mpp_payment_gate,
            )),
        )
        .route("/v2/session", post(crate::routes::v2::create_session))
        .route("/v2/auth/verify", post(crate::routes::v2::verify_auth))
        .route(
            "/v1/jobs",
            post(crate::routes::jobs::create_job).route_layer(middleware::from_fn_with_state(
                state.clone(),
                crate::mpp::middleware::mpp_payment_gate,
            )),
        )
        .route("/v1/jobs/{id}", get(crate::routes::jobs::get_job))
        .route("/v1/vms/{id}", get(crate::routes::vm::get_vm))
        .route("/v1/vms/{id}", delete(crate::routes::vm::delete_vm))
        .with_state(state)
}

/// Helper to read a response body as JSON.
async fn body_json(response: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 64)
        .await
        .expect("should read body");
    serde_json::from_slice(&bytes).expect("body should be valid JSON")
}

/// Helper to read a response body as a Vec<u8>.
#[allow(dead_code)]
async fn body_bytes(response: axum::http::Response<Body>) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), 1024 * 64)
        .await
        .expect("should read body")
        .to_vec()
}

/// Build an MPP credential header value for dev-mode testing.
fn dev_mode_credential(challenge_id: &str) -> String {
    let cred = serde_json::json!({
        "challenge_id": challenge_id,
        "tx_hash": "0xfake_test_tx_hash_1234567890abcdef",
        "network": "tempo",
        "payer": "0xTestPayer1234567890abcdef12345678",
        "signature": "0xfakesig"
    });
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_string(&cred).unwrap());
    format!("Payment {}", encoded)
}

/// Extract the MPP challenge from a 402 response body and return its id + base64 encoding.
fn extract_challenge_from_body(body: &serde_json::Value) -> (String, String) {
    let challenge_id = body["id"].as_str().expect("challenge should have id");
    let challenge_b64 = base64::engine::general_purpose::STANDARD
        .encode(serde_json::to_string(body).unwrap());
    (challenge_id.to_string(), challenge_b64)
}

// ===========================================================================
// 1. GET /health
// ===========================================================================

#[tokio::test]
async fn test_health_returns_200_with_status_ok() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["service"], "mpp-hosting");
    assert!(json["version"].is_string());
}

// ===========================================================================
// 2. GET /status
// ===========================================================================

#[tokio::test]
async fn test_status_returns_200_with_capacity_info() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert!(json["available_slots"].is_number());
    assert!(json["running_vms"].is_number());
    assert!(json["total_vcpus"].is_number());
    assert!(json["available_vcpus"].is_number());
    assert!(json["total_ram_mb"].is_number());
    assert!(json["available_ram_mb"].is_number());
    // With no running VMs, all resources should be available
    assert_eq!(json["running_vms"], 0);
}

// ===========================================================================
// 3. GET /.well-known/x402
// ===========================================================================

#[tokio::test]
async fn test_well_known_x402_returns_discovery_json() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/x402")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["version"], 1);
    assert!(json["resources"].is_array());
    let resources = json["resources"].as_array().unwrap();
    assert!(!resources.is_empty());
    // Should mention the provision endpoint
    assert!(resources
        .iter()
        .any(|r| r.as_str().unwrap_or("").contains("/v1/provision")));
}

// ===========================================================================
// 4. GET /openapi.json
// ===========================================================================

#[tokio::test]
async fn test_openapi_json_returns_valid_spec() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["openapi"], "3.0.3");
    assert!(json["info"]["title"].is_string());
    assert!(json["paths"].is_object());
    assert!(json["paths"]["/v1/provision"].is_object());
    assert!(json["paths"]["/v1/provision"]["post"].is_object());
    // Check x-payment-info exists on the provision endpoint
    assert!(json["paths"]["/v1/provision"]["post"]["x-payment-info"].is_object());
    assert!(json["paths"]["/health"].is_object());
    assert!(json["paths"]["/status"].is_object());
}

#[tokio::test]
async fn test_openapi_json_contains_payment_networks() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = body_json(response).await;
    let payment_info = &json["paths"]["/v1/provision"]["post"]["x-payment-info"];
    let networks = &payment_info["networks"];
    // Should have Base, Celo, and Tempo networks
    assert!(networks["eip155:8453"].is_object(), "Base network missing");
    assert!(networks["eip155:42220"].is_object(), "Celo network missing");
    assert!(networks["tempo:4217"].is_object(), "Tempo network missing");
}

// ===========================================================================
// 5. POST /v1/provision (no payment) -- returns 402
// ===========================================================================

#[tokio::test]
async fn test_provision_no_payment_returns_402() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vcpus": 1,
                        "ram_mb": 512,
                        "disk_gb": 10,
                        "duration": 3600
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    // Check that the www-authenticate header is present (MPP challenge)
    assert!(
        response.headers().contains_key("www-authenticate"),
        "Missing www-authenticate header"
    );

    // Check that the PAYMENT-REQUIRED header is present (x402)
    let has_payment_required = response.headers().contains_key("payment-required")
        || response.headers().contains_key("PAYMENT-REQUIRED");
    assert!(has_payment_required, "Missing PAYMENT-REQUIRED header");

    // Validate the body is a valid MPP challenge JSON
    let json = body_json(response).await;
    assert!(json["id"].is_string(), "Challenge should have an id");
    assert_eq!(json["realm"], "mpp-hosting");
    assert_eq!(json["method"], "tempo");
    assert_eq!(json["intent"], "charge");
    assert!(json["amount"].is_string(), "Challenge should have an amount");
    assert!(
        json["recipient"].is_string(),
        "Challenge should have a recipient"
    );
    assert!(
        json["expires_at"].is_string(),
        "Challenge should have an expiry"
    );
    assert!(
        json["signature"].is_string(),
        "Challenge should be signed"
    );
}

#[tokio::test]
async fn test_provision_no_payment_empty_body_returns_402() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    // Should still return a valid challenge with default pricing
    let json = body_json(response).await;
    assert!(json["id"].is_string());
}

#[tokio::test]
async fn test_provision_402_has_x402_payment_required_header() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    // Decode the PAYMENT-REQUIRED header (base64-encoded JSON)
    let pr_header = response
        .headers()
        .get("payment-required")
        .expect("PAYMENT-REQUIRED header should exist");
    let pr_b64 = pr_header.to_str().unwrap();
    let pr_bytes = base64::engine::general_purpose::STANDARD
        .decode(pr_b64)
        .expect("PAYMENT-REQUIRED header should be valid base64");
    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_bytes).expect("Decoded content should be valid JSON");

    // Validate x402 structure
    assert_eq!(pr_json["x402Version"], 2);
    assert!(pr_json["resource"].is_object());
    assert!(pr_json["accepts"].is_array());
    let accepts = pr_json["accepts"].as_array().unwrap();
    assert!(
        accepts.len() >= 2,
        "Should accept at least Base and Celo networks"
    );
}

// ===========================================================================
// 6. GET /v1/provision -- returns 402 for x402 scanner probes
// ===========================================================================

#[tokio::test]
async fn test_provision_get_returns_402() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/provision")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    // The middleware intercepts and returns 402 for GET too (no payment headers)
    assert!(
        response.headers().contains_key("www-authenticate"),
        "GET /v1/provision should include MPP www-authenticate header"
    );
}

// ===========================================================================
// 7. POST /v1/provision with dev-mode MPP credential
// ===========================================================================

#[tokio::test]
async fn test_provision_with_dev_mode_payment_reaches_handler() {
    // Step 1: Get a challenge by sending a request without payment
    let app = test_app();
    let challenge_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vcpus": 1,
                        "ram_mb": 512,
                        "disk_gb": 10,
                        "duration": 3600
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(challenge_response.status(), StatusCode::PAYMENT_REQUIRED);
    let challenge_body = body_json(challenge_response).await;
    let (challenge_id, challenge_b64) = extract_challenge_from_body(&challenge_body);

    // Step 2: Send the credential with the challenge
    let app = test_app();
    let credential_header = dev_mode_credential(&challenge_id);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .header("authorization", &credential_header)
                .header("x-mpp-challenge", &challenge_b64)
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vcpus": 1,
                        "ram_mb": 512,
                        "disk_gb": 10,
                        "duration": 3600
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    // In dev mode with valid challenge, the payment gate passes through.
    // The handler will fail because Firecracker isn't available, so we expect 500.
    // But crucially, the middleware allowed us through (not 402).
    assert_ne!(
        status,
        StatusCode::PAYMENT_REQUIRED,
        "Middleware should not return 402 when valid payment credential is provided"
    );

    // Should either be 201 (unlikely without Firecracker) or 500
    assert!(
        status == StatusCode::CREATED || status == StatusCode::INTERNAL_SERVER_ERROR,
        "Expected 201 or 500, got {}",
        status
    );

    // If we got through, there should be a payment-receipt header
    if status == StatusCode::CREATED || status == StatusCode::INTERNAL_SERVER_ERROR {
        // The middleware attaches the receipt before the handler response is built,
        // so it should be present even on 500 (middleware wraps the response).
        // Actually, the receipt is only added on the success path of the middleware,
        // so it will be there.
        if response.headers().contains_key("payment-receipt") {
            let receipt_b64 = response
                .headers()
                .get("payment-receipt")
                .unwrap()
                .to_str()
                .unwrap();
            let receipt_bytes = base64::engine::general_purpose::STANDARD
                .decode(receipt_b64)
                .expect("Receipt should be valid base64");
            let receipt_json: serde_json::Value =
                serde_json::from_slice(&receipt_bytes).expect("Receipt should be valid JSON");
            assert_eq!(receipt_json["status"], "success");
            assert_eq!(receipt_json["challenge_id"], challenge_id);
        }
    }
}

#[tokio::test]
async fn test_provision_bad_credential_format_returns_400() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .header("authorization", "Payment not-valid-base64!!!")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_provision_credential_without_challenge_header_returns_400() {
    let app = test_app();
    let credential_header = dev_mode_credential("some-challenge-id");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .header("authorization", &credential_header)
                // No x-mpp-challenge header
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_provision_replay_challenge_returns_conflict() {
    // Step 1: Get a challenge
    let app = test_app();
    let challenge_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let challenge_body = body_json(challenge_response).await;
    let (challenge_id, challenge_b64) = extract_challenge_from_body(&challenge_body);

    // Step 2: Use the challenge the first time (will get 500 or 201)
    let app = test_app();
    let credential_header = dev_mode_credential(&challenge_id);
    let _first = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .header("authorization", &credential_header)
                .header("x-mpp-challenge", &challenge_b64)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Step 3: Replay the same challenge — should get 409 Conflict
    // Note: Each call to test_app() creates a new DB, so we need a shared app.
    // We must rebuild the test to use a single DB instance.
    // Actually, test_app() creates a fresh in-memory DB each time, so the
    // replay won't be detected across separate apps.
    // For this test, we need to share the state. Let's build it manually.

    let config = test_config();
    let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).expect("IP pool"));
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    let build_router = |state: AppState| {
        Router::new()
            .route(
                "/v1/provision",
                post(crate::routes::provision::provision)
                    .get(crate::routes::provision::provision_info)
                    .route_layer(middleware::from_fn_with_state(
                        state.clone(),
                        crate::mpp::middleware::mpp_payment_gate,
                    )),
            )
            .with_state(state)
    };

    // Get challenge from shared state
    let app = build_router(state.clone());
    let challenge_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    let challenge_body = body_json(challenge_response).await;
    let (challenge_id, challenge_b64) = extract_challenge_from_body(&challenge_body);
    let credential_header = dev_mode_credential(&challenge_id);

    // First use
    let app = build_router(state.clone());
    let _first = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .header("authorization", &credential_header)
                .header("x-mpp-challenge", &challenge_b64)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Replay
    let app = build_router(state.clone());
    let replay = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .header("authorization", &credential_header)
                .header("x-mpp-challenge", &challenge_b64)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        replay.status(),
        StatusCode::CONFLICT,
        "Replayed challenge should return 409 Conflict"
    );
}

// ===========================================================================
// 8. GET /v1/vms/{id} -- returns VM info or 404
// ===========================================================================

#[tokio::test]
async fn test_get_vm_not_found() {
    let app = test_app();
    let fake_id = uuid::Uuid::new_v4();
    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/v1/vms/{}", fake_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert!(json["error"].is_string());
}

#[tokio::test]
async fn test_get_vm_invalid_uuid_returns_400() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/vms/not-a-valid-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"], "invalid_vm_id");
}

#[tokio::test]
async fn test_get_vm_returns_inserted_vm() {
    // Insert a VM directly into the DB and then query it via the API
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Running,
        vcpus: 2,
        ram_mb: 1024,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: Some("tap0".to_string()),
        socket_path: Some("/tmp/test.sock".to_string()),
        pid: Some(12345),
        payment_tx: Some("0xtesttx".to_string()),
        price_micro: 5000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: None,
    };
    db.insert_vm(&record).unwrap();

    let app = Router::new()
        .route("/v1/vms/{id}", get(crate::routes::vm::get_vm))
        .with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/v1/vms/{}", vm_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["vm_id"], vm_id.to_string());
    assert_eq!(json["status"], "running");
    assert_eq!(json["vcpus"], 2);
    assert_eq!(json["ram_mb"], 1024);
    assert_eq!(json["disk_gb"], 10);
    assert_eq!(json["image"], "ubuntu-24.04");
    assert_eq!(json["ip"], "172.16.0.5");
}

// ===========================================================================
// 9. DELETE /v1/vms/{id} -- returns terminated or 404
// ===========================================================================

#[tokio::test]
async fn test_delete_vm_not_found() {
    let app = test_app();
    let fake_id = uuid::Uuid::new_v4();
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(&format!("/v1/vms/{}", fake_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_delete_vm_invalid_uuid_returns_400() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/v1/vms/not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"], "invalid_vm_id");
}

// ===========================================================================
// 10. POST /v1/jobs (no payment) -- returns 402
// ===========================================================================

#[tokio::test]
async fn test_create_job_no_payment_returns_402() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/jobs")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "command": "echo hello",
                        "timeout": 300
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(
        response.headers().contains_key("www-authenticate"),
        "Jobs endpoint should return MPP challenge"
    );
    let has_payment_required = response.headers().contains_key("payment-required")
        || response.headers().contains_key("PAYMENT-REQUIRED");
    assert!(
        has_payment_required,
        "Jobs endpoint should return x402 PAYMENT-REQUIRED header"
    );
}

// ===========================================================================
// 11. GET /v1/jobs/{id} -- returns job info or 404
// ===========================================================================

#[tokio::test]
async fn test_get_job_not_found() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/jobs/nonexistent-job-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"], "not_found");
}

#[tokio::test]
async fn test_get_job_returns_inserted_job() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    let now = chrono::Utc::now();
    let job = crate::db::models::JobRecord {
        id: "test-job-123".to_string(),
        vm_id: None,
        status: "pending".to_string(),
        command: "echo hello world".to_string(),
        setup_script: None,
        output: String::new(),
        exit_code: None,
        timeout_secs: 300,
        vcpus: 1,
        ram_mb: 512,
        created_at: now,
        started_at: None,
        completed_at: None,
        expires_at: now + chrono::Duration::minutes(5),
        payment_tx: Some("0xtestjobtx".to_string()),
        price_micro: 1000,
    };
    db.insert_job(&job).unwrap();

    let app = Router::new()
        .route("/v1/jobs/{id}", get(crate::routes::jobs::get_job))
        .with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/jobs/test-job-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["job_id"], "test-job-123");
    assert_eq!(json["status"], "pending");
    assert_eq!(json["command"], "echo hello world");
}

// ===========================================================================
// 12. POST /v2/provision (no payment) -- returns 402
// ===========================================================================

#[tokio::test]
async fn test_v2_provision_no_payment_returns_402() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/provision")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vcpus": 1,
                        "ram_mb": 512,
                        "disk_gb": 10,
                        "duration": 3600
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(
        response.headers().contains_key("www-authenticate"),
        "V2 provision should return MPP challenge"
    );
}

// ===========================================================================
// 13. POST /v2/session -- session creation without valid VM
// ===========================================================================

#[tokio::test]
async fn test_v2_session_invalid_vm_id_returns_400() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": "not-a-uuid",
                        "signature": "0xabc123",
                        "address": "0x1234"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"], "invalid_vm_id");
}

#[tokio::test]
async fn test_v2_session_nonexistent_vm_returns_404() {
    let app = test_app();
    let fake_vm_id = uuid::Uuid::new_v4();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": fake_vm_id.to_string(),
                        "signature": "0xabc123",
                        "address": "0x1234"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"], "vm_not_found");
}

#[tokio::test]
async fn test_v2_session_terminated_vm_returns_conflict() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    // Insert a terminated VM
    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Terminated,
        vcpus: 1,
        ram_mb: 512,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: None,
        socket_path: None,
        pid: None,
        payment_tx: None,
        price_micro: 1000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: Some(now),
    };
    db.insert_vm(&record).unwrap();

    let app = Router::new()
        .route("/v2/session", post(crate::routes::v2::create_session))
        .with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": vm_id.to_string(),
                        "signature": "0xabc123",
                        "address": "0x1234"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"], "vm_not_running");
}

#[tokio::test]
async fn test_v2_session_running_vm_without_v2_auth_returns_400() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    // Insert a running VM without v2 auth data (provisioned via v1)
    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Running,
        vcpus: 1,
        ram_mb: 512,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: None,
        socket_path: None,
        pid: None,
        payment_tx: None,
        price_micro: 1000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: None,
    };
    db.insert_vm(&record).unwrap();

    let app = Router::new()
        .route("/v2/session", post(crate::routes::v2::create_session))
        .with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": vm_id.to_string(),
                        "signature": "0xabc123",
                        "address": "0xTestPayer1234567890abcdef12345678"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"], "no_v2_auth");
}

#[tokio::test]
async fn test_v2_session_address_mismatch_returns_403() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    // Insert a running VM with v2 auth data
    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Running,
        vcpus: 1,
        ram_mb: 512,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: None,
        socket_path: None,
        pid: None,
        payment_tx: None,
        price_micro: 1000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: None,
    };
    db.insert_vm(&record).unwrap();
    db.update_vm_v2_auth(
        &vm_id,
        "0xCorrectPayer1234567890abcdef12345678",
        "openvps:test:challenge123",
    )
    .unwrap();

    let app = Router::new()
        .route("/v2/session", post(crate::routes::v2::create_session))
        .with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": vm_id.to_string(),
                        "signature": "0xabc123",
                        "address": "0xWrongAddress0000000000000000000000"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let json = body_json(response).await;
    assert_eq!(json["error"], "address_mismatch");
}

#[tokio::test]
async fn test_v2_session_valid_returns_token() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let payer = "0xCorrectPayer1234567890abcdef12345678";
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Running,
        vcpus: 1,
        ram_mb: 512,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: None,
        socket_path: None,
        pid: None,
        payment_tx: None,
        price_micro: 1000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: None,
    };
    db.insert_vm(&record).unwrap();
    db.update_vm_v2_auth(&vm_id, payer, "openvps:test:challenge123")
        .unwrap();

    let app = Router::new()
        .route("/v2/session", post(crate::routes::v2::create_session))
        .with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": vm_id.to_string(),
                        "signature": "0xabcdef1234567890",
                        "address": payer
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["token"].is_string());
    assert!(!json["token"].as_str().unwrap().is_empty());
    assert!(json["ssh_command"].is_string());
    assert!(json["ssh_host"].is_string());
    assert!(json["ssh_port"].is_number());
    assert!(json["expires_in_seconds"].is_number());
}

// ===========================================================================
// 14. POST /v2/auth/verify -- token verification
// ===========================================================================

#[tokio::test]
async fn test_v2_auth_verify_invalid_token() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "token": "nonexistent-invalid-token"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["authorized"], false);
    assert!(json["vm_id"].is_null());
}

#[tokio::test]
async fn test_v2_auth_verify_valid_token_consumed() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    // Insert a VM so the FK constraint is satisfied
    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Running,
        vcpus: 1,
        ram_mb: 512,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: None,
        socket_path: None,
        pid: None,
        payment_tx: None,
        price_micro: 1000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: None,
    };
    db.insert_vm(&record).unwrap();

    // Directly create a session token in the DB
    let token = "test-one-time-token-abc123";
    let expires = now + chrono::Duration::hours(1);
    db.create_session_token(token, &vm_id.to_string(), "0xTestPayer", &expires)
        .unwrap();

    let build_app = |state: AppState| {
        Router::new()
            .route("/v2/auth/verify", post(crate::routes::v2::verify_auth))
            .with_state(state)
    };

    // First verification should succeed
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "token": token
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["authorized"], true);
    assert_eq!(json["vm_id"], vm_id.to_string());

    // Second verification should fail (token already consumed)
    let app = build_app(state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "token": token
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["authorized"], false);
}

// ===========================================================================
// Additional edge case tests
// ===========================================================================

#[tokio::test]
async fn test_unknown_route_returns_404() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/nonexistent/path")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // axum returns 404 for unmatched routes
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_provision_402_challenge_has_correct_pricing() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vcpus": 2,
                        "ram_mb": 1024,
                        "disk_gb": 10,
                        "duration": 7200
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let json = body_json(response).await;

    // Calculate expected price manually:
    // hours = 7200/3600 = 2.0
    // cpu: 5000 * 2 * 2.0 = 20000
    // ram: 2 * 1024 * 2.0 = 4096
    // disk: 100 * 10 * 2.0 = 2000
    // total = ceil(26096) = 26096
    let expected_price = 26096u64;
    let challenge_amount: u64 = json["amount"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(challenge_amount, expected_price);
}

#[tokio::test]
async fn test_provision_402_www_authenticate_format() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/provision")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let www_auth = response
        .headers()
        .get("www-authenticate")
        .unwrap()
        .to_str()
        .unwrap();

    // Should follow the MPP format: Payment id="...", realm="...", ...
    assert!(www_auth.starts_with("Payment "), "www-authenticate should start with 'Payment'");
    assert!(www_auth.contains("realm=\"mpp-hosting\""));
    assert!(www_auth.contains("method=\"tempo\""));
    assert!(www_auth.contains("intent=\"charge\""));
    assert!(www_auth.contains("request=\""));
    assert!(www_auth.contains("expires=\""));
}

#[tokio::test]
async fn test_v2_session_invalid_signature_format_returns_400() {
    let config = test_config();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let ip_pool = Arc::new(IpPool::new(&config.vm_subnet, &[]).unwrap());
    let vm_manager = Arc::new(VmManager::new(config.clone(), db.clone(), ip_pool));
    let state = AppState {
        config: Arc::new(config),
        vm_manager,
    };

    let vm_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let payer = "0xCorrectPayer1234567890abcdef12345678";
    let record = crate::db::models::VmRecord {
        id: vm_id,
        status: crate::db::models::VmStatus::Running,
        vcpus: 1,
        ram_mb: 512,
        disk_gb: 10,
        image: "ubuntu-24.04".to_string(),
        ip_addr: Some("172.16.0.5".to_string()),
        ssh_port: Some(22),
        tap_device: None,
        socket_path: None,
        pid: None,
        payment_tx: None,
        price_micro: 1000,
        created_at: now,
        expires_at: now + chrono::Duration::hours(1),
        terminated_at: None,
    };
    db.insert_vm(&record).unwrap();
    db.update_vm_v2_auth(&vm_id, payer, "openvps:test:challenge123")
        .unwrap();

    let app = Router::new()
        .route("/v2/session", post(crate::routes::v2::create_session))
        .with_state(state);

    // Signature without 0x prefix should fail
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/session")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "vm_id": vm_id.to_string(),
                        "signature": "no-hex-prefix",
                        "address": payer
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"], "invalid_signature");
}

#[tokio::test]
async fn test_jobs_402_challenge_body_structure() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/jobs")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "command": "echo test",
                        "vcpus": 2,
                        "ram_mb": 1024,
                        "timeout": 600
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let json = body_json(response).await;

    // The challenge body should have valid MPP fields
    assert!(json["id"].is_string());
    assert_eq!(json["realm"], "mpp-hosting");
    assert!(json["recipient"].is_string());
    assert!(json["signature"].is_string());
    assert!(json["expires_at"].is_string());
}

#[tokio::test]
async fn test_health_no_authentication_required() {
    // Health endpoint should work without any headers
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_status_no_authentication_required() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_openapi_no_authentication_required() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_v2_auth_verify_empty_token() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v2/auth/verify")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "token": ""
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["authorized"], false);
}
