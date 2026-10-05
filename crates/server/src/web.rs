//! HTTP API for the web interface (`website/konto`, see `docs/ACCOUNTS.md`).
//!
//! Listens on a private address (default 127.0.0.1:21380); Caddy forwards
//! `https://<website>/api/*` to it. The browser does all cryptography itself:
//! it sends only values derived from the password and gets sealed keys and
//! encrypted blobs back, exactly like the app. A sign-in gives an HttpOnly,
//! SameSite=Strict session cookie scoped to `/api`.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD};
use base64::Engine;
use ctxremote_proto::account::{AccountError, AccountOp, AccountReply, Kdf, LoginSetup};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::Server;

const COOKIE: &str = "ctx_session";
/// A session ends after this long without a request …
const IDLE: Duration = Duration::from_secs(12 * 3600);
/// … and after this long in any case.
const ABSOLUTE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Signed-in browsers: SHA-256 of the cookie → session. In memory only, so a
/// server restart signs everyone out of the web interface.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<[u8; 32], Session>>);

struct Session {
    account: u64,
    created: Instant,
    seen: Instant,
    /// Random bytes the browser XORs its account key with before keeping it
    /// in sessionStorage, so the page survives a reload. What the browser
    /// stores is worthless once this session ends (logout, expiry, password
    /// change, server restart).
    pad: [u8; 32],
}

impl Sessions {
    /// A new session: its cookie value and its pad.
    fn start(&self, account: u64) -> (String, [u8; 32]) {
        let token: [u8; 32] = rand::random();
        let pad: [u8; 32] = rand::random();
        let now = Instant::now();
        let mut sessions = self.0.lock().unwrap();
        sessions.retain(|_, s| now.duration_since(s.seen) < IDLE && now.duration_since(s.created) < ABSOLUTE);
        sessions.insert(Sha256::digest(token).into(), Session { account, created: now, seen: now, pad });
        (URL_SAFE_NO_PAD.encode(token), pad)
    }

    fn account(&self, headers: &HeaderMap) -> Option<u64> {
        self.session(headers).map(|(account, _)| account)
    }

    fn session(&self, headers: &HeaderMap) -> Option<(u64, [u8; 32])> {
        let token = URL_SAFE_NO_PAD.decode(cookie(headers)?).ok()?;
        let hash: [u8; 32] = Sha256::digest(token).into();
        let now = Instant::now();
        let mut sessions = self.0.lock().unwrap();
        let session = sessions.get_mut(&hash)?;
        if now.duration_since(session.seen) >= IDLE || now.duration_since(session.created) >= ABSOLUTE {
            sessions.remove(&hash);
            return None;
        }
        session.seen = now;
        Some((session.account, session.pad))
    }

    fn end(&self, headers: &HeaderMap) {
        if let Some(token) = cookie(headers).and_then(|c| URL_SAFE_NO_PAD.decode(c).ok()) {
            let hash: [u8; 32] = Sha256::digest(token).into();
            self.0.lock().unwrap().remove(&hash);
        }
    }

    /// Ends every session of `account`, e.g. after its password changed.
    fn end_all(&self, account: u64, except: Option<&HeaderMap>) {
        let keep = except.and_then(cookie).and_then(|c| URL_SAFE_NO_PAD.decode(c).ok()).map(|t| <[u8; 32]>::from(Sha256::digest(t)));
        self.0.lock().unwrap().retain(|hash, s| s.account != account || Some(*hash) == keep);
    }
}

fn cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

fn session_cookie(token: &str) -> HeaderValue {
    let max_age = ABSOLUTE.as_secs();
    HeaderValue::from_str(&format!("{COOKIE}={token}; Path=/api; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}"))
        .expect("cookie is ASCII")
}

fn cleared_cookie() -> HeaderValue {
    HeaderValue::from_static("ctx_session=; Path=/api; HttpOnly; Secure; SameSite=Strict; Max-Age=0")
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<AccountError> for ApiError {
    fn from(e: AccountError) -> Self {
        let status = match e {
            AccountError::WrongPassword | AccountError::NotLinked => StatusCode::UNAUTHORIZED,
            AccountError::Conflict => StatusCode::CONFLICT,
            AccountError::RateLimited | AccountError::Locked => StatusCode::TOO_MANY_REQUESTS,
            AccountError::Storage => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        };
        Self(status, e.to_string())
    }
}

fn bad(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

type ApiResult<T> = Result<T, ApiError>;

/// Bytes as standard base64 in JSON.
mod b64 {
    use super::*;
    use serde::Deserializer;

    pub fn de_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        B64.decode(text).map_err(serde::de::Error::custom)
    }

    pub fn de_array<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        de_vec(d)?.try_into().map_err(|_| serde::de::Error::custom("falsche Länge"))
    }
}

fn enc(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct KdfJson {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

impl From<Kdf> for KdfJson {
    fn from(k: Kdf) -> Self {
        Self { memory_kib: k.memory_kib, iterations: k.iterations, parallelism: k.parallelism }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupJson {
    email: String,
    #[serde(deserialize_with = "b64::de_array")]
    salt: [u8; 16],
    kdf: KdfJson,
    #[serde(deserialize_with = "b64::de_array")]
    auth: [u8; 32],
    #[serde(deserialize_with = "b64::de_vec")]
    wrapped: Vec<u8>,
    #[serde(deserialize_with = "b64::de_array")]
    recovery_auth: [u8; 32],
    #[serde(deserialize_with = "b64::de_vec")]
    recovery_wrapped: Vec<u8>,
}

impl From<SetupJson> for LoginSetup {
    fn from(s: SetupJson) -> Self {
        LoginSetup {
            email: s.email,
            salt: s.salt,
            kdf: Kdf { memory_kib: s.kdf.memory_kib, iterations: s.kdf.iterations, parallelism: s.kdf.parallelism },
            auth: s.auth,
            wrapped: s.wrapped,
            recovery_auth: s.recovery_auth,
            recovery_wrapped: s.recovery_wrapped,
        }
    }
}

#[derive(Clone)]
struct Api {
    server: Arc<Server>,
    sessions: Arc<Sessions>,
    /// The website's origin; other origins are refused (besides SameSite cookies).
    origin: Option<String>,
}

/// Serves the API on `addr` until the process ends.
pub async fn serve(server: Arc<Server>, addr: SocketAddr, origin: Option<String>) -> anyhow::Result<()> {
    let api = Api { server, sessions: Arc::default(), origin };
    let app = Router::new()
        .route("/api/prelogin", post(prelogin))
        .route("/api/register", post(register))
        .route("/api/login", post(login))
        .route("/api/recover", post(recover))
        .route("/api/logout", post(logout))
        .route("/api/account", get(account))
        .route("/api/session-pad", get(session_pad))
        .route("/api/book", get(get_book).put(put_book))
        .route("/api/devices", get(devices))
        .route("/api/devices/{key}", delete(remove_device))
        .route("/api/login-setup", post(set_login))
        .route("/api/pairing", post(pairing))
        .route("/api/verify", post(verify))
        .route("/api/verify/resend", post(resend_verification))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(api);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Web-API lauscht auf {addr}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

impl Api {
    /// The client's address as Caddy (and Cloudflare in front of it) report it.
    fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        header("cf-connecting-ip")
            .or_else(|| header("x-forwarded-for").and_then(|v| v.split(',').next().map(|s| s.trim().to_string())))
            .and_then(|v| v.parse().ok())
            .unwrap_or(peer.ip())
    }

    /// Every request: rate limit, and for changes a JSON body from our own origin.
    fn guard(&self, method: &Method, headers: &HeaderMap, peer: SocketAddr) -> ApiResult<()> {
        if !self.server.allow_connect(Self::client_ip(headers, peer)) {
            return Err(AccountError::RateLimited.into());
        }
        if method != Method::GET {
            if let (Some(expected), Some(origin)) = (&self.origin, headers.get(header::ORIGIN)) {
                if origin.to_str().ok() != Some(expected.as_str()) {
                    return Err(ApiError(StatusCode::FORBIDDEN, "Fremde Herkunft".into()));
                }
            }
        }
        Ok(())
    }

    fn signed_in(&self, headers: &HeaderMap) -> ApiResult<u64> {
        self.sessions
            .account(headers)
            .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Bitte erneut anmelden".into()))
    }

    fn op(&self, account: u64, op: AccountOp) -> ApiResult<AccountReply> {
        let reply = self.server.accounts.lock().unwrap().handle_web(account, op)?;
        Ok(self.server.with_presence(reply))
    }

    fn signed_in_response(&self, account: u64, mut body: serde_json::Value) -> Response {
        let (token, pad) = self.sessions.start(account);
        body["pad"] = json!(enc(&pad));
        let mut response = Json(body).into_response();
        response.headers_mut().insert(header::SET_COOKIE, session_cookie(&token));
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

#[derive(Deserialize)]
struct EmailJson {
    email: String,
}

async fn prelogin(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<EmailJson>,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::POST, &headers, peer)?;
    // Same answer as for devices, including made-up salts for unknown addresses.
    let reply = api.server.accounts.lock().unwrap().handle([0; 32], AccountOp::Prelogin { email: body.email })?;
    let AccountReply::Prelogin { salt, kdf } = reply else { return Err(bad("unerwartet")) };
    Ok(Json(json!({ "salt": enc(&salt), "kdf": KdfJson::from(kdf) })))
}

async fn register(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<SetupJson>,
) -> ApiResult<Response> {
    api.guard(&Method::POST, &headers, peer)?;
    let account = api.server.accounts.lock().unwrap().register_web(body.into())?;
    info!(account, "Konto im Web angelegt");
    Ok(api.signed_in_response(account, json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginJson {
    email: String,
    #[serde(deserialize_with = "b64::de_array")]
    auth: [u8; 32],
}

async fn login(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginJson>,
) -> ApiResult<Response> {
    api.guard(&Method::POST, &headers, peer)?;
    let (account, wrapped) = api.server.accounts.lock().unwrap().login_web(&body.email, &body.auth, false)?;
    Ok(api.signed_in_response(account, json!({ "wrapped": enc(&wrapped) })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoverJson {
    email: String,
    #[serde(deserialize_with = "b64::de_array")]
    recovery_auth: [u8; 32],
}

async fn recover(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<RecoverJson>,
) -> ApiResult<Response> {
    api.guard(&Method::POST, &headers, peer)?;
    let (account, wrapped) = api.server.accounts.lock().unwrap().login_web(&body.email, &body.recovery_auth, true)?;
    Ok(api.signed_in_response(account, json!({ "wrapped": enc(&wrapped) })))
}

async fn logout(State(api): State<Api>, headers: HeaderMap) -> Response {
    api.sessions.end(&headers);
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(header::SET_COOKIE, cleared_cookie());
    response
}

/// The pad of this session, for a page that was reloaded.
async fn session_pad(State(api): State<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Response> {
    api.guard(&Method::GET, &headers, peer)?;
    let (_, pad) = api
        .sessions
        .session(&headers)
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Bitte erneut anmelden".into()))?;
    let mut response = Json(json!({ "pad": enc(&pad) })).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn account(State(api): State<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::GET, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    let AccountReply::LoginStatus(login) = api.op(id, AccountOp::LoginStatus)? else { return Err(bad("unerwartet")) };
    let AccountReply::Status(info) = api.op(id, AccountOp::Status)? else { return Err(bad("unerwartet")) };
    Ok(Json(json!({
        "email": login.as_ref().map(|l| l.email.clone()),
        "verified": login.is_some_and(|l| l.verified),
        "devices": info.map_or(0, |i| i.devices),
    })))
}

async fn get_book(State(api): State<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::GET, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    let AccountReply::Book { revision, blob } = api.op(id, AccountOp::GetBook)? else { return Err(bad("unerwartet")) };
    Ok(Json(json!({ "revision": revision, "blob": enc(&blob) })))
}

#[derive(Deserialize)]
struct PutBookJson {
    base: u64,
    #[serde(deserialize_with = "b64::de_vec")]
    blob: Vec<u8>,
}

async fn put_book(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PutBookJson>,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::PUT, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    let AccountReply::Stored { revision } = api.op(id, AccountOp::PutBook { base: body.base, blob: body.blob })? else {
        return Err(bad("unerwartet"));
    };
    Ok(Json(json!({ "revision": revision })))
}

async fn devices(State(api): State<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::GET, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    let AccountReply::Devices(members) = api.op(id, AccountOp::Devices)? else { return Err(bad("unerwartet")) };
    let list: Vec<_> = members
        .into_iter()
        .map(|m| {
            json!({
                "publicKey": hex::encode(m.public_key),
                "id": m.device.map(|d| d.to_string()),
                "online": m.online,
                "label": enc(&m.label),
            })
        })
        .collect();
    Ok(Json(json!(list)))
}

async fn remove_device(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::DELETE, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    let public_key: [u8; 32] = hex::decode(&key).ok().and_then(|k| k.try_into().ok()).ok_or_else(|| bad("ungültiges Gerät"))?;
    api.op(id, AccountOp::RemoveDevice { public_key })?;
    Ok(Json(json!({ "ok": true })))
}

/// A new password or address: other browser sessions end, this one stays.
async fn set_login(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<SetupJson>,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::POST, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    api.op(id, AccountOp::SetLogin { login: body.into() })?;
    api.sessions.end_all(id, Some(&headers));
    debug!(account = id, "Anmeldung im Web geändert");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairingJson {
    code_id: String,
    #[serde(deserialize_with = "b64::de_array")]
    salt: [u8; 16],
    #[serde(deserialize_with = "b64::de_array")]
    verifier: [u8; 32],
    #[serde(deserialize_with = "b64::de_vec")]
    sealed: Vec<u8>,
}

/// A pairing code shown in the browser, for adding a device to the account.
async fn pairing(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PairingJson>,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::POST, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    let op = AccountOp::OfferPairing { code_id: body.code_id, salt: body.salt, verifier: body.verifier, sealed: body.sealed };
    api.op(id, op)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TokenJson {
    token: String,
}

/// The link from the confirmation mail; needs no session.
async fn verify(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<TokenJson>,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::POST, &headers, peer)?;
    api.server.accounts.lock().unwrap().verify_email(&body.token).map_err(|_| {
        ApiError(StatusCode::BAD_REQUEST, "Der Link ist ungültig oder abgelaufen. Lassen Sie sich im Konto einen neuen schicken.".into())
    })?;
    Ok(Json(json!({ "ok": true })))
}

async fn resend_verification(
    State(api): State<Api>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    api.guard(&Method::POST, &headers, peer)?;
    let id = api.signed_in(&headers)?;
    api.server.accounts.lock().unwrap().resend_verification(id)?;
    Ok(Json(json!({ "ok": true })))
}
