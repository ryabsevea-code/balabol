use axum::{
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

const RECORD_TTL_SECS: u64 = 120; // 2 minutes TTL

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnounceRequest {
    pub public_key_hex: String,
    pub endpoint: String, // "ip:port" or multiaddr
    pub nat_type: String, // "open" | "cone" | "symmetric" | "unknown"
    pub timestamp_ms: u64,
    pub signature_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    pub public_key_hex: String,
    pub endpoint: String,
    pub observed_ip: String,
    pub nat_type: String,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnounceResponse {
    pub success: bool,
    pub observed_addr: String,
    pub ttl_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchLookupRequest {
    pub public_keys: Vec<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub peers: Arc<DashMap<String, PeerRecord>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("balabol_rendezvous=debug,tower_http=info")
        .init();

    let state = AppState {
        peers: Arc::new(DashMap::new()),
    };

    // Background task: Evict expired records every 30 seconds
    let eviction_peers = state.peers.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let current = now_ms();
            let threshold = current.saturating_sub(RECORD_TTL_SECS * 1000);
            let before = eviction_peers.len();
            eviction_peers.retain(|_, record| record.last_seen_ms >= threshold);
            let evicted = before.saturating_sub(eviction_peers.len());
            if evicted > 0 {
                info!("Evicted {} stale peers, active peers count: {}", evicted, eviction_peers.len());
            }
        }
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/api/v1/announce", post(announce_handler))
        .route("/api/v1/lookup/{pubkey}", get(lookup_handler))
        .route("/api/v1/batch-lookup", post(batch_lookup_handler))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 4001));
    info!("🚀 Balabol Stateless Rendezvous Server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "balabol-rendezvous",
        "active_peers": state.peers.len(),
        "timestamp": now_ms(),
    }))
}

async fn announce_handler(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    Json(payload): Json<AnnounceRequest>,
) -> Result<Json<AnnounceResponse>, (StatusCode, String)> {
    let clean_pk = payload.public_key_hex.strip_prefix("bala:").unwrap_or(&payload.public_key_hex);

    // Verify key length
    let pk_bytes = hex::decode(clean_pk)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid public key hex format".into()))?;
    if pk_bytes.len() != 32 {
        return Err((StatusCode::BAD_REQUEST, "Public key must be 32 bytes".into()));
    }

    let mut pk_arr = [0u8; 32];
    pk_arr.copy_from_slice(&pk_bytes);
    let verifying_key = VerifyingKey::from_bytes(&pk_arr)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid Ed25519 public key".into()))?;

    // Verify signature
    let sig_bytes = hex::decode(&payload.signature_hex)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid signature hex format".into()))?;
    if sig_bytes.len() != 64 {
        return Err((StatusCode::BAD_REQUEST, "Signature must be 64 bytes".into()));
    }
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = Signature::from_bytes(&sig_arr);

    // Reconstruct canonical message that was signed: `${endpoint}:${timestamp_ms}:${nat_type}`
    let message = format!("{}:{}:{}", payload.endpoint, payload.timestamp_ms, payload.nat_type);
    if verifying_key.verify(message.as_bytes(), &signature).is_err() {
        warn!("Announce rejected: signature verification failed for {}", clean_pk);
        return Err((StatusCode::UNAUTHORIZED, "Signature verification failed".into()));
    }

    let current = now_ms();
    // Prevent replay attacks (timestamp within +/- 5 minutes)
    if payload.timestamp_ms > current + 300_000 || payload.timestamp_ms < current.saturating_sub(300_000) {
        return Err((StatusCode::BAD_REQUEST, "Timestamp drifted too far from server clock".into()));
    }

    let observed_ip = addr.ip().to_string();
    let record = PeerRecord {
        public_key_hex: clean_pk.to_string(),
        endpoint: payload.endpoint,
        observed_ip: observed_ip.clone(),
        nat_type: payload.nat_type,
        last_seen_ms: current,
    };

    state.peers.insert(clean_pk.to_string(), record);

    Ok(Json(AnnounceResponse {
        success: true,
        observed_addr: addr.to_string(),
        ttl_secs: RECORD_TTL_SECS,
    }))
}

async fn lookup_handler(
    Path(pubkey): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<PeerRecord>, StatusCode> {
    let clean_pk = pubkey.strip_prefix("bala:").unwrap_or(&pubkey);
    if let Some(record) = state.peers.get(clean_pk) {
        Ok(Json(record.clone()))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn batch_lookup_handler(
    State(state): State<AppState>,
    Json(payload): Json<BatchLookupRequest>,
) -> Json<Vec<PeerRecord>> {
    let mut found = Vec::new();
    for pk in payload.public_keys {
        let clean_pk = pk.strip_prefix("bala:").unwrap_or(&pk);
        if let Some(record) = state.peers.get(clean_pk) {
            found.push(record.clone());
        }
    }
    Json(found)
}
