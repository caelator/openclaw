use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use clap::Parser;
use common::protocol::{
    ConnectParams, EventFrame, GatewayFrame, HelloOk, HelloOkAuth, HelloOkFeatures, HelloOkPolicy,
    HelloOkServer, RequestFrame,
};
use common::session::SessionEntry;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tracing::{error, info, warn};
use uuid::Uuid;


use common::session::SessionStore;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 18789)]
    port: u16,
    #[arg(long, default_value = "sessions.json")]
    session_store: String,
    #[arg(long, default_value = "config.json")]
    config: String,
}

#[derive(Clone)]
struct AppState {
    sessions: SessionStore,
    config: Arc<common::config::OpenClawConfig>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let port = args.port;

    // Load config
    let config_content = std::fs::read_to_string(&args.config).unwrap_or_else(|_| "{}".to_string());
    let config: common::config::OpenClawConfig = serde_json::from_str(&config_content).unwrap_or_default();
    let config = Arc::new(config);

    // Start Telegram
    let sessions = SessionStore::new(&args.session_store);
    let sessions_clone = sessions.clone();
    
    let config_clone = config.clone();
    tokio::spawn(async move {
        if let Err(e) = telegram::start(&config_clone, sessions_clone).await {
            error!("Telegram error: {}", e);
        }
    });

    // Start Discord
    let sessions_clone = sessions.clone();
    let config_clone = config.clone();
    tokio::spawn(async move {
        if let Err(e) = discord::start(&config_clone, sessions_clone).await {
            error!("Discord error: {}", e);
        }
    });

    // Start WhatsApp
    let sessions_clone = sessions.clone();
    let config_clone = config.clone();
    tokio::spawn(async move {
        if let Err(e) = whatsapp::start(&config_clone, sessions_clone).await {
            error!("WhatsApp error: {}", e);
        }
    });


    let state = AppState { 
        sessions,
        config: config.clone(),
    };

    let app = Router::new()
        .route("/", get(ws_handler))
        .route("/debug/sessions", get(list_sessions))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    info!("Gateway listening on {}", addr);
    info!("Session store: {}", args.session_store);
    info!("Config loaded from: {}", args.config);

    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn list_sessions(State(state): State<AppState>) -> Json<Vec<SessionEntry>> {
    Json(state.sessions.list().await)
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let conn_id = Uuid::new_v4().to_string();
    info!("New connection: {}", conn_id);

    // 1. Send connect.challenge
    let nonce = Uuid::new_v4().to_string();
    let challenge = GatewayFrame::Event(EventFrame {
        event: "connect.challenge".to_string(),
        payload: Some(serde_json::json!({
            "nonce": nonce,
            "ts": chrono::Utc::now().timestamp_millis(),
        })),
        seq: None,
    });

    if let Err(e) = socket
        .send(Message::Text(serde_json::to_string(&challenge).unwrap()))
        .await
    {
        error!("Failed to send challenge: {}", e);
        return;
    }

    // 2. Wait for connect request
    // TODO: Implement timeout
    let mut authenticated = false;

    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                match serde_json::from_str::<GatewayFrame>(&text) {
                    Ok(GatewayFrame::Request(req)) => {
                        if !authenticated {
                            if req.method == "connect" {
                                if handle_connect(&mut socket, req, &conn_id, &state).await {
                                    authenticated = true;
                                } else {
                                    return; // Connect failed
                                }
                            } else {
                                warn!("Expected connect request, got method: {}", req.method);
                                // TODO: Send error response
                                return;
                            }
                        } else {
                            // Already authenticated, handle other requests
                            info!("Received request: {} (id: {})", req.method, req.id);
                            // TODO: Dispatch request
                        }
                    }
                    Ok(GatewayFrame::Event(evt)) => {
                         info!("Received event: {}", evt.event);
                    }
                     Ok(GatewayFrame::Response(res)) => {
                         info!("Received response: {} (ok: {})", res.id, res.ok);
                    }
                    Ok(GatewayFrame::HelloOk(_)) => {
                        warn!("Received unexpected HelloOk from client");
                    }
                    Err(e) => {
                        error!("Failed to parse message: {}", e);
                    }
                }
            }
            Ok(Message::Close(c)) => {
                info!("Client disconnected: {:?}", c);
                return;
            }
            Err(e) => {
                error!("Socket error: {}", e);
                return;
            }
            _ => {}
        }
    }
}

async fn handle_connect(
    socket: &mut WebSocket, 
    req: RequestFrame, 
    conn_id: &str,
    state: &AppState
) -> bool {
    let params: ConnectParams = match req.params {
        Some(p) => match serde_json::from_value(p) {
            Ok(p) => p,
            Err(e) => {
                error!("Invalid connect params: {}", e);
                return false;
            }
        },
        None => {
             error!("Missing connect params");
             return false;
        }
    };

    info!(
        "Client connected: {} ({}) [{}]",
        params.client.id,
        params.client.display_name.as_deref().unwrap_or("?"),
        params.client.version
    );

    // Check for existing session
    let client_id = &params.client.id;
    let session_key = client_id.clone(); // For now, use client_id as session_key (simplification)
    
    let mut session = state.sessions.get(&session_key).await;

    if session.is_none() {
        // Create new session
        let new_session = SessionEntry {
             session_id: Uuid::new_v4().to_string(),
             updated_at: chrono::Utc::now().timestamp_millis(),
             session_file: None,
             spawned_by: None,
             spawn_depth: None,
             system_sent: None,
             chat_type: Some("direct".to_string()), // Default to direct
             provider_override: None,
             model_override: None,
             label: params.client.display_name.clone(),
             display_name: params.client.display_name.clone(),
             channel: Some("websocket".to_string()),
             group_id: None,
             subject: None,
             group_channel: None,
             origin: None,
             delivery_context: None,
             input_tokens: Some(0),
             output_tokens: Some(0),
             total_tokens: Some(0),
             extra: std::collections::HashMap::new(),
         };
         
         if let Err(e) = state.sessions.update(session_key.clone(), new_session.clone()).await {
             error!("Failed to create session: {}", e);
             return false;
         }
         info!("Created new session for {}", client_id);
         session = Some(new_session);
    } else {
        info!("Restored session for {}", client_id);
    }

    // TODO: Authenticate device/token

    let hello = GatewayFrame::HelloOk(HelloOk {
        protocol: 3, // PROTOCOL_VERSION
        server: HelloOkServer {
            version: "0.1.0".to_string(),
            commit: None,
            host: Some("localhost".to_string()),
            conn_id: conn_id.to_string(),
        },
        features: HelloOkFeatures {
            methods: vec![],
            events: vec![],
        },
        snapshot: serde_json::json!({}), // Empty snapshot
        canvas_host_url: None,
        auth: Some(HelloOkAuth {
            device_token: "dummy-token".to_string(), // TODO: Real token
            role: params.role.unwrap_or_else(|| "operator".to_string()),
            scopes: params.scopes.unwrap_or_default(),
            issued_at_ms: Some(chrono::Utc::now().timestamp_millis() as u64),
        }),
        policy: HelloOkPolicy {
            max_payload: 1024 * 1024,
            max_buffered_bytes: 1024 * 1024,
            tick_interval_ms: 1000,
        },
    });

    if let Err(e) = socket
        .send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
    {
        error!("Failed to send hello: {}", e);
        return false;
    }

    true
}
