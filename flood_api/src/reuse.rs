//! Opt-in result reuse for the model runs (/region, /flood, /timeline, /evacuation, /layers): every
//! successful run is stored under `<FLOOD_OUT>/reuse/<route>/<name>.json` with its request; a later
//! request with `"reuse": true`, the same name and otherwise the same body gets the stored answer at
//! once (plus `"reused": true` and header `x-reused: 1`) instead of a new run. Without the flag, or
//! when anything in the request differs, the model runs as before and the stored answer is replaced.
use axum::{body::{to_bytes, Body, Bytes}, extract::{Request, State}, http::{header, HeaderValue, Method, StatusCode},
           middleware::Next, response::{IntoResponse, Response}};
use serde_json::{json, Value};
use std::path::PathBuf;

const ROUTES: [&str; 5] = ["/region", "/flood", "/timeline", "/evacuation", "/layers"];
const MAX_BODY: usize = 4 << 20;

pub async fn layer(State(dir): State<PathBuf>, req: Request, next: Next) -> Response {
    let route = req.uri().path().to_owned();
    if req.method() != Method::POST || !ROUTES.contains(&route.as_str()) {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_BODY).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, axum::Json(json!({"error": "request body too large"}))).into_response();
    };
    // the request without the flag is the key; anything that is not a named JSON object just runs
    let mut key: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let wants = key.get("reuse") == Some(&Value::Bool(true));
    let name = key.get("name").and_then(Value::as_str).filter(|n| crate::models::check_name(n).is_ok()).map(str::to_owned);
    if let Some(o) = key.as_object_mut() {
        o.remove("reuse");
    }
    let path = name.map(|n| dir.join(route.trim_start_matches('/')).join(format!("{n}.json")));
    if let (true, Some(p)) = (wants, &path) {
        let stored = tokio::fs::read(p).await.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if let Some(mut s) = stored.filter(|s| s["request"] == key) {
            if let Some(o) = s["response"].as_object_mut() {
                o.insert("reused".into(), Value::Bool(true));
            }
            let mut res = axum::Json(s["response"].take()).into_response();
            res.headers_mut().insert("x-reused", HeaderValue::from_static("1"));
            return res;
        }
    }
    let res = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let Some(p) = path.filter(|_| res.status() == StatusCode::OK) else { return res };
    let (parts, body) = res.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap_or_else(|_| Bytes::new());
    if let Ok(response) = serde_json::from_slice::<Value>(&bytes) {
        let tmp = p.with_extension("tmp");
        let text = json!({"request": key, "response": response}).to_string();
        // best effort: a failed store only means the next reuse runs the model again
        if tokio::fs::create_dir_all(p.parent().unwrap()).await.is_ok() && tokio::fs::write(&tmp, text).await.is_ok() {
            let _ = tokio::fs::rename(&tmp, &p).await;
        }
    }
    let mut res = Response::from_parts(parts, Body::from(bytes));
    res.headers_mut().entry(header::CONTENT_TYPE).or_insert(HeaderValue::from_static("application/json"));
    res
}
