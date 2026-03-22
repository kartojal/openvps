use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::AppState;

/// GET /.well-known/x402 — x402 discovery endpoint for 402scan and other indexers.
pub async fn well_known_x402() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "version": 1,
            "resources": [
                "POST /v1/provision"
            ]
        })),
    )
}

/// GET /openapi.json — OpenAPI spec with x-payment-info for x402 discovery.
pub async fn openapi(State(state): State<AppState>) -> impl IntoResponse {
    let recipient = &state.config.payment_recipient;
    let base_asset = &state.config.x402_base_asset;
    let celo_asset = &state.config.x402_celo_asset;
    let usdc_contract = &state.config.usdc_contract;

    (
        StatusCode::OK,
        Json(json!({
            "openapi": "3.0.3",
            "info": {
                "title": "OpenVPS",
                "description": "AI-agent VPS hosting. Pay with stablecoins via x402 or MPP. Get root SSH access to Ubuntu 24.04 Firecracker microVMs in seconds.",
                "version": "0.1.0",
                "contact": {
                    "url": "https://openvps.sh"
                },
                "guidance": "To provision a VM: POST /v1/provision with desired specs. You'll receive a 402 with payment instructions. Pay via x402 (Base USDC, Celo USDC) or MPP (Tempo USDC.e/pathUSD), then resubmit with payment proof. Full instructions at https://openvps.sh/skill.md"
            },
            "servers": [
                { "url": "https://openvps.sh" }
            ],
            "x-discovery": {
                "protocols": ["x402", "mpp"],
                "ownershipProofs": [recipient]
            },
            "paths": {
                "/v1/provision": {
                    "post": {
                        "summary": "Provision a Firecracker microVM",
                        "description": "Request a VPS with specified resources. Returns 402 with payment challenge. After payment, returns VM details with SSH access.",
                        "x-payment-info": {
                            "protocols": ["x402", "mpp"],
                            "pricingMode": "quote",
                            "description": "Price depends on vCPUs, RAM, disk, and duration requested",
                            "networks": {
                                "eip155:8453": {
                                    "asset": base_asset,
                                    "assetName": "USDC",
                                    "decimals": 6,
                                    "recipient": recipient
                                },
                                "eip155:42220": {
                                    "asset": celo_asset,
                                    "assetName": "USDC",
                                    "decimals": 6,
                                    "recipient": recipient
                                },
                                "tempo:4217": {
                                    "asset": usdc_contract,
                                    "assetName": "USDC.e",
                                    "decimals": 6,
                                    "recipient": recipient
                                }
                            }
                        },
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "vcpus": { "type": "integer", "minimum": 1, "maximum": 4, "default": 1 },
                                            "ram_mb": { "type": "integer", "minimum": 256, "maximum": 4096, "default": 512 },
                                            "disk_gb": { "type": "integer", "minimum": 1, "maximum": 20, "default": 10 },
                                            "image": { "type": "string", "default": "ubuntu-24.04", "enum": ["ubuntu-24.04"] },
                                            "duration": { "type": "integer", "minimum": 60, "maximum": 86400, "default": 3600, "description": "Duration in seconds" }
                                        }
                                    }
                                }
                            }
                        },
                        "responses": {
                            "201": {
                                "description": "VM provisioned successfully",
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "type": "object",
                                            "properties": {
                                                "vm_id": { "type": "string" },
                                                "ip": { "type": "string" },
                                                "ssh_host": { "type": "string" },
                                                "ssh_port": { "type": "integer" },
                                                "ssh_command": { "type": "string" },
                                                "expires_at": { "type": "string", "format": "date-time" },
                                                "status": { "type": "string" },
                                                "ssh_private_key": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            },
                            "402": {
                                "description": "Payment required. Includes PAYMENT-REQUIRED header (x402) and WWW-Authenticate header (MPP)."
                            }
                        }
                    }
                },
                "/v1/vms/{id}": {
                    "get": {
                        "summary": "Get VM status",
                        "parameters": [
                            { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }
                        ],
                        "responses": {
                            "200": { "description": "VM details" }
                        }
                    },
                    "delete": {
                        "summary": "Terminate a VM",
                        "parameters": [
                            { "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }
                        ],
                        "responses": {
                            "200": { "description": "VM terminated" }
                        }
                    }
                },
                "/health": {
                    "get": {
                        "summary": "Health check",
                        "responses": { "200": { "description": "Service healthy" } }
                    }
                },
                "/status": {
                    "get": {
                        "summary": "Live capacity status",
                        "responses": { "200": { "description": "Available slots, running VMs, free resources" } }
                    }
                }
            }
        })),
    )
}
