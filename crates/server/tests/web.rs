//! The web interface's HTTP API against a real server. The browser's
//! cryptography is replaced by fixed values here; `web/` tests that part.

mod common;

use common::*;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ORIGIN: &str = "https://konto.example";

/// One HTTP/1.1 request; returns status, Set-Cookie and the JSON body.
async fn call(http: &str, method: &str, path: &str, cookie: Option<&str>, origin: &str, body: Option<Value>) -> (u16, Option<String>, Value) {
    let mut stream = tokio::net::TcpStream::connect(http).await.unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\nOrigin: {origin}\r\n");
    if let Some(cookie) = cookie {
        request.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    if !body.is_empty() {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let cookie = head
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: ").or_else(|| l.strip_prefix("Set-Cookie: ")))
        .map(|c| c.split(';').next().unwrap().to_string());
    // Small bodies come in one chunk or plain; take the JSON part.
    let json_start = body.find(['{', '[']).unwrap_or(0);
    let json_end = body.rfind(['}', ']']).map_or(body.len(), |i| i + 1);
    (status, cookie, serde_json::from_str(&body[json_start..json_end]).unwrap_or(Value::Null))
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn setup(email: &str, auth: u8) -> Value {
    json!({
        "email": email,
        "salt": b64(&[3; 16]),
        "kdf": { "memoryKib": 65536, "iterations": 3, "parallelism": 1 },
        "auth": b64(&[auth; 32]),
        "wrapped": b64(&[auth; 40]),
        "recoveryAuth": b64(&[auth + 100; 32]),
        "recoveryWrapped": b64(&[auth + 100; 40]),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn web_account_round_trip() {
    let (_server, _addr, http) = start_server_with_web(Some(ORIGIN)).await;
    let http = http.unwrap();

    // Not signed in.
    let (status, _, _) = call(&http, "GET", "/api/book", None, ORIGIN, None).await;
    assert_eq!(status, 401);

    let (status, cookie, _) = call(&http, "POST", "/api/register", None, ORIGIN, Some(setup("web@example.org", 1))).await;
    assert_eq!(status, 200);
    let cookie = cookie.expect("Sitzungs-Cookie");
    let (status, _, body) = call(&http, "POST", "/api/register", None, ORIGIN, Some(setup("WEB@example.org", 2))).await;
    assert_eq!(status, 400, "{body}");

    // Another origin is refused.
    let (status, _, _) = call(&http, "POST", "/api/register", None, "https://evil.example", Some(setup("x@example.org", 3))).await;
    assert_eq!(status, 403);

    // Book: write, conflict, read.
    let (status, _, body) = call(&http, "PUT", "/api/book", Some(&cookie), ORIGIN, Some(json!({ "base": 0, "blob": b64(b"abc") }))).await;
    assert_eq!((status, body["revision"].as_u64()), (200, Some(1)));
    let (status, _, _) = call(&http, "PUT", "/api/book", Some(&cookie), ORIGIN, Some(json!({ "base": 0, "blob": b64(b"x") }))).await;
    assert_eq!(status, 409);
    let (_, _, body) = call(&http, "GET", "/api/book", Some(&cookie), ORIGIN, None).await;
    assert_eq!(body["blob"], json!(b64(b"abc")));

    // Prelogin knows the salt; login returns the sealed key and a new session.
    let (_, _, body) = call(&http, "POST", "/api/prelogin", None, ORIGIN, Some(json!({ "email": "Web@Example.org" }))).await;
    assert_eq!(body["salt"], json!(b64(&[3; 16])));
    let wrong = json!({ "email": "web@example.org", "auth": b64(&[9; 32]) });
    assert_eq!(call(&http, "POST", "/api/login", None, ORIGIN, Some(wrong)).await.0, 401);
    let right = json!({ "email": "web@example.org", "auth": b64(&[1; 32]) });
    let (status, second, body) = call(&http, "POST", "/api/login", None, ORIGIN, Some(right)).await;
    assert_eq!((status, body["wrapped"].clone()), (200, json!(b64(&[1; 40]))));
    let second = second.unwrap();
    // A reloaded page gets the same pad back with its cookie, and only then.
    let pad = body["pad"].as_str().expect("pad").to_string();
    let (status, _, body) = call(&http, "GET", "/api/session-pad", Some(&second), ORIGIN, None).await;
    assert_eq!((status, body["pad"].as_str()), (200, Some(pad.as_str())));
    assert_eq!(call(&http, "GET", "/api/session-pad", None, ORIGIN, None).await.0, 401);

    let (_, _, body) = call(&http, "GET", "/api/account", Some(&second), ORIGIN, None).await;
    assert_eq!(body["email"], json!("web@example.org"));
    let (_, _, body) = call(&http, "GET", "/api/devices", Some(&second), ORIGIN, None).await;
    assert_eq!(body, json!([]));

    // A new password ends the other sessions, not this one.
    let (status, _, _) = call(&http, "POST", "/api/login-setup", Some(&second), ORIGIN, Some(setup("web@example.org", 5))).await;
    assert_eq!(status, 200);
    assert_eq!(call(&http, "GET", "/api/book", Some(&cookie), ORIGIN, None).await.0, 401);
    assert_eq!(call(&http, "GET", "/api/book", Some(&second), ORIGIN, None).await.0, 200);

    // Recovery with the code's value.
    let recover = json!({ "email": "web@example.org", "recoveryAuth": b64(&[105; 32]) });
    let (status, _, body) = call(&http, "POST", "/api/recover", None, ORIGIN, Some(recover)).await;
    assert_eq!((status, body["wrapped"].clone()), (200, json!(b64(&[105; 40]))));

    // Logout ends the session.
    let (status, cleared, _) = call(&http, "POST", "/api/logout", Some(&second), ORIGIN, None).await;
    assert_eq!(status, 200);
    assert_eq!(cleared.as_deref(), Some("ctx_session="));
    assert_eq!(call(&http, "GET", "/api/book", Some(&second), ORIGIN, None).await.0, 401);
    assert_eq!(call(&http, "GET", "/api/session-pad", Some(&second), ORIGIN, None).await.0, 401);

    // Deleting needs the password once more, then everything is gone.
    let (_, third, _) = call(&http, "POST", "/api/login", None, ORIGIN, Some(json!({ "email": "web@example.org", "auth": b64(&[5; 32]) }))).await;
    let third = third.expect("Sitzung");
    let wrong = json!({ "auth": b64(&[9; 32]) });
    assert_eq!(call(&http, "POST", "/api/account/delete", Some(&third), ORIGIN, Some(wrong)).await.0, 401);
    let right = json!({ "auth": b64(&[5; 32]) });
    let (status, cleared, _) = call(&http, "POST", "/api/account/delete", Some(&third), ORIGIN, Some(right.clone())).await;
    assert_eq!((status, cleared.as_deref()), (200, Some("ctx_session=")));
    assert_eq!(call(&http, "GET", "/api/book", Some(&third), ORIGIN, None).await.0, 401);
    let again = json!({ "email": "web@example.org", "auth": b64(&[5; 32]) });
    assert_eq!(call(&http, "POST", "/api/login", None, ORIGIN, Some(again)).await.0, 401);
    // The address is free again.
    let (status, _, _) = call(&http, "POST", "/api/register", None, ORIGIN, Some(setup("web@example.org", 7))).await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_session_needs_the_password_to_change_the_login() {
    let (_server, _addr, http) = start_server_with_env(Some(ORIGIN), &[("CTXREMOTE_WEB_FRESH_SECS", "0")]).await;
    let http = http.unwrap();
    let (_, cookie, _) = call(&http, "POST", "/api/register", None, ORIGIN, Some(setup("alt@example.org", 1))).await;
    let cookie = cookie.expect("Sitzungs-Cookie");

    // A stolen cookie alone cannot take the account over.
    let (status, _, _) = call(&http, "POST", "/api/login-setup", Some(&cookie), ORIGIN, Some(setup("dieb@example.org", 2))).await;
    assert_eq!(status, 401);
    let mut wrong = setup("dieb@example.org", 2);
    wrong["current"] = json!(b64(&[9; 32]));
    assert_eq!(call(&http, "POST", "/api/login-setup", Some(&cookie), ORIGIN, Some(wrong)).await.0, 401);
    // The owner knows the password; the session stays valid.
    let mut right = setup("alt@example.org", 2);
    right["current"] = json!(b64(&[1; 32]));
    assert_eq!(call(&http, "POST", "/api/login-setup", Some(&cookie), ORIGIN, Some(right)).await.0, 200);
    assert_eq!(call(&http, "GET", "/api/book", Some(&cookie), ORIGIN, None).await.0, 200);
    let login = json!({ "email": "alt@example.org", "auth": b64(&[2; 32]) });
    assert_eq!(call(&http, "POST", "/api/login", None, ORIGIN, Some(login)).await.0, 200);
}
