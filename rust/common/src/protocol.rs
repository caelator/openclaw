use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum GatewayFrame {
    #[serde(rename = "req")]
    Request(RequestFrame),
    #[serde(rename = "res")]
    Response(ResponseFrame),
    #[serde(rename = "event")]
    Event(EventFrame),
    #[serde(rename = "hello-ok")]
    HelloOk(HelloOk),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConnectParams {
    #[serde(rename = "minProtocol")]
    pub min_protocol: u32,
    #[serde(rename = "maxProtocol")]
    pub max_protocol: u32,
    pub client: GatewayClient,
    #[serde(default)]
    pub caps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    // permissions: Option<HashMap<String, bool>>, // TODO
    #[serde(rename = "pathEnv", default, skip_serializing_if = "Option::is_none")]
    pub path_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<GatewayDevice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<GatewayAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(rename = "userAgent", default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GatewayClient {
    pub id: String,
    #[serde(rename = "displayName", default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub version: String,
    pub platform: String,
    #[serde(rename = "deviceFamily", default, skip_serializing_if = "Option::is_none")]
    pub device_family: Option<String>,
    #[serde(rename = "modelIdentifier", default, skip_serializing_if = "Option::is_none")]
    pub model_identifier: Option<String>,
    pub mode: String, // "host" | "control-ui" | "cli" etc.
    #[serde(rename = "instanceId", default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GatewayDevice {
    pub id: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    pub signature: String,
    #[serde(rename = "signedAt")]
    pub signed_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GatewayAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RequestFrame {
    pub id: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResponseFrame {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorShape>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EventFrame {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ErrorShape {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(rename = "retryAfterMs", default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HelloOk {
    pub protocol: u32,
    pub server: HelloOkServer,
    pub features: HelloOkFeatures,
    pub snapshot: Value, // TODO: Define Snapshot struct
    #[serde(rename = "canvasHostUrl", default, skip_serializing_if = "Option::is_none")]
    pub canvas_host_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<HelloOkAuth>,
    pub policy: HelloOkPolicy,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HelloOkServer {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(rename = "connId")]
    pub conn_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HelloOkFeatures {
    pub methods: Vec<String>,
    pub events: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HelloOkAuth {
    #[serde(rename = "deviceToken")]
    pub device_token: String,
    pub role: String,
    pub scopes: Vec<String>,
    #[serde(rename = "issuedAtMs", default, skip_serializing_if = "Option::is_none")]
    pub issued_at_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HelloOkPolicy {
    #[serde(rename = "maxPayload")]
    pub max_payload: u32,
    #[serde(rename = "maxBufferedBytes")]
    pub max_buffered_bytes: u32,
    #[serde(rename = "tickIntervalMs")]
    pub tick_interval_ms: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TickEvent {
    pub ts: u64,
}
