//! Opt-in background mode for the slow routes (/region in a new area, /help/zones, big /flood or
//! /evacuation runs): `?async=1` on any POST returns 202 `{job, status_url}` at once and the
//! request runs on; `GET /jobs/{id}` answers 202 `{"status":"running"}` until it is done, then
//! the route's own status and JSON body. Results live in memory for KEEP (lost on restart).
use crate::extract::PathArg;
use axum::{body::{to_bytes, Bytes}, extract::{Request, State}, http::{header, Method, StatusCode},
           middleware::Next, response::{IntoResponse, Response}, Json};
use serde_json::json;
use std::{collections::HashMap, sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

const KEEP: Duration = Duration::from_secs(6 * 3600);

#[derive(Clone, Default)]
pub struct Jobs {
    store: Arc<Mutex<HashMap<String, (Instant, Option<(StatusCode, Bytes)>)>>>,
    next: Arc<AtomicU64>,
}

fn wants_async(req: &Request) -> bool {
    req.method() == Method::POST
        && req.uri().query().is_some_and(|q| q.split('&').any(|p| p == "async=1" || p == "async=true"))
}

pub async fn layer(State(jobs): State<Jobs>, req: Request, next: Next) -> Response {
    if !wants_async(&req) {
        return next.run(req).await;
    }
    // counter + clock bits: not guessable from another job's id
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
    let id = format!("{}-{nanos:08x}", jobs.next.fetch_add(1, Ordering::Relaxed) + 1);
    {
        let mut store = jobs.store.lock().unwrap();
        store.retain(|_, (t, done)| done.is_none() || t.elapsed() < KEEP);
        store.insert(id.clone(), (Instant::now(), None));
    }
    let (store, key) = (jobs.store.clone(), id.clone());
    tokio::spawn(async move {
        let res = next.run(req).await;
        let status = res.status();
        let body = to_bytes(res.into_body(), usize::MAX).await
            .unwrap_or_else(|e| Bytes::from(json!({"error": format!("reading result: {e}")}).to_string()));
        store.lock().unwrap().insert(key, (Instant::now(), Some((status, body))));
    });
    (StatusCode::ACCEPTED, Json(json!({"job": id, "status": "running", "status_url": format!("/jobs/{id}")}))).into_response()
}

pub async fn get(State(jobs): State<Jobs>, PathArg(id): PathArg<String>) -> Response {
    match jobs.store.lock().unwrap().get(&id) {
        None => (StatusCode::NOT_FOUND, Json(json!({"error": format!("no job {id} (finished jobs are kept 6 h, not across restarts)")}))).into_response(),
        Some((t, None)) => (StatusCode::ACCEPTED, Json(json!({"job": id, "status": "running", "running_s": t.elapsed().as_secs()}))).into_response(),
        Some((_, Some((status, body)))) => (*status, [(header::CONTENT_TYPE, "application/json")], body.clone()).into_response(),
    }
}
