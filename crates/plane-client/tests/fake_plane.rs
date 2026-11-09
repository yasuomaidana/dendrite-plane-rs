//! A tiny in-process Plane: just enough HTTP for the client's tests, with request recording.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path_and_query: String,
    pub api_key: Option<String>,
    pub body: String,
}

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub delay: Duration,
}

pub fn json(body: &str) -> Reply {
    Reply {
        status: 200,
        body: body.to_string(),
        delay: Duration::ZERO,
    }
}

pub struct Fake {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Req>>>,
    pub peak_in_flight: Arc<AtomicUsize>,
}

/// Where the fake Plane serves the current user, as real Plane versions differ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UserRoute {
    /// `/api/v1/users/me/`, the public API.
    #[default]
    V1,
    /// Only `/api/users/me/`, as some self-hosted versions do.
    InternalOnly,
    /// Neither.
    Missing,
}

#[derive(Debug, Clone)]
pub struct FakeState {
    pub user_route: UserRoute,
    pub user: serde_json::Value,
    pub labels: Vec<serde_json::Value>,
    pub issues: HashMap<String, serde_json::Value>,
    pub comments: HashMap<String, Vec<serde_json::Value>>,
}

impl Default for FakeState {
    fn default() -> Self {
        FakeState {
            user_route: UserRoute::V1,
            user: serde_json::json!({
                "id": "usr-1",
                "email": "agent@example.com",
                "display_name": "Agent Dendrite"
            }),
            labels: Vec::new(),
            issues: HashMap::new(),
            comments: HashMap::new(),
        }
    }
}

impl Fake {
    pub fn start(handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(handler);
        let (reqs, fl, pk) = (requests.clone(), in_flight, peak.clone());
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (handler, reqs, fl, pk) =
                    (handler.clone(), reqs.clone(), fl.clone(), pk.clone());
                thread::spawn(move || serve(stream, &*handler, &reqs, &fl, &pk));
            }
        });
        Fake {
            url,
            requests,
            peak_in_flight: peak,
        }
    }

    /// Starts a fake Plane with standard routes for issues, labels, comments, and current user.
    pub fn with_routes(initial: FakeState) -> (Fake, Arc<Mutex<FakeState>>) {
        let state = Arc::new(Mutex::new(initial));
        let s = state.clone();
        let fake = Fake::start(move |req| handle_routes(req, &s));
        (fake, state)
    }

    pub fn recorded(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }
}

type State = Arc<Mutex<FakeState>>;

fn status(code: u16) -> Reply {
    Reply {
        status: code,
        body: if code == 404 { "{}" } else { "bad request" }.into(),
        delay: Duration::ZERO,
    }
}

fn parse_body(req: &Req) -> serde_json::Value {
    serde_json::from_str(&req.body).unwrap_or_default()
}

pub fn handle_routes(req: &Req, state_lock: &State) -> Reply {
    let mut state = state_lock.lock().unwrap();
    let path = req.path_and_query.split('?').next().unwrap_or("");
    route_user(req, path, &state)
        .or_else(|| route_labels(req, path, &mut state))
        .or_else(|| route_comments(req, path, &mut state))
        .or_else(|| route_issues(req, path, &mut state))
        .unwrap_or_else(|| status(404))
}

fn route_user(req: &Req, path: &str, state: &FakeState) -> Option<Reply> {
    let user_path = match state.user_route {
        UserRoute::V1 => "/api/v1/users/me/",
        UserRoute::InternalOnly => "/api/users/me/",
        UserRoute::Missing => return None,
    };
    if path != user_path {
        return None;
    }
    if req.api_key.as_deref() == Some("wrong-key") {
        return Some(Reply {
            status: 401,
            body: "unauthorized".into(),
            delay: Duration::ZERO,
        });
    }
    Some(json(&state.user.to_string()))
}

fn route_labels(req: &Req, path: &str, state: &mut FakeState) -> Option<Reply> {
    if !path.ends_with("/labels/") {
        return None;
    }
    match req.method.as_str() {
        "GET" => Some(json(
            &serde_json::json!({ "results": state.labels }).to_string(),
        )),
        "POST" => {
            let Ok(body) = serde_json::from_str::<serde_json::Value>(&req.body) else {
                return Some(status(400));
            };
            let label = serde_json::json!({
                "id": format!("lbl-{}", state.labels.len() + 1),
                "name": body["name"].as_str().unwrap_or(""),
                "color": body["color"].as_str().unwrap_or("#7C3AED"),
            });
            state.labels.push(label.clone());
            Some(json(&label.to_string()))
        }
        _ => None,
    }
}

fn route_comments(req: &Req, path: &str, state: &mut FakeState) -> Option<Reply> {
    let idx = path.find("/issues/")?;
    let (issue_id, comment_part) = path[idx + "/issues/".len()..].split_once("/comments/")?;
    if !state.issues.contains_key(issue_id) {
        return Some(status(404));
    }
    let cid = comment_part.trim_matches('/');
    let entry = state.comments.entry(issue_id.to_string()).or_default();
    match (req.method.as_str(), cid.is_empty()) {
        ("GET", true) => Some(json(&serde_json::json!({ "results": entry }).to_string())),
        ("POST", true) => {
            let n = entry.len();
            let stamp = format!("2026-10-28T12:00:{n:02}Z");
            entry.push(serde_json::json!({
                "id": format!("c-{}", n + 1),
                "comment_html": parse_body(req)["comment_html"].as_str().unwrap_or(""),
                "created_at": stamp,
                "updated_at": stamp,
            }));
            Some(json("{}"))
        }
        ("PATCH", false) => Some(match entry.iter_mut().find(|c| c["id"] == cid) {
            Some(c) => {
                if let Some(html) = parse_body(req)["comment_html"].as_str() {
                    c["comment_html"] = html.into();
                    c["updated_at"] = "2026-10-28T12:30:00Z".into();
                }
                json("{}")
            }
            None => status(404),
        }),
        _ => None,
    }
}

fn route_issues(req: &Req, path: &str, state: &mut FakeState) -> Option<Reply> {
    let idx = path.find("/issues/")?;
    let id = path[idx + "/issues/".len()..].trim_matches('/');
    if id.is_empty() {
        if req.method != "GET" {
            return None;
        }
        let list: Vec<serde_json::Value> = state.issues.values().cloned().collect();
        return Some(json(
            &serde_json::json!({ "results": list, "next_page_results": false, "next_cursor": "" })
                .to_string(),
        ));
    }
    let Some(iss) = state.issues.get_mut(id) else {
        return Some(status(404));
    };
    match req.method.as_str() {
        "GET" => Some(json(&iss.to_string())),
        "PATCH" => {
            for (k, v) in parse_body(req).as_object().into_iter().flatten() {
                iss[k] = v.clone();
            }
            Some(json(&iss.to_string()))
        }
        _ => None,
    }
}

fn serve(
    stream: TcpStream,
    handler: &dyn Fn(&Req) -> Reply,
    reqs: &Mutex<Vec<Req>>,
    in_flight: &AtomicUsize,
    peak: &AtomicUsize,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let (mut api_key, mut len) = (None, 0usize);
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
            break;
        }
        let lower = h.to_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("x-api-key:") {
            api_key = Some(h["x-api-key:".len()..].trim().to_string());
        }
    }
    let mut body = vec![0u8; len];
    let _ = reader.read_exact(&mut body);
    let req = Req {
        method,
        path_and_query: target,
        api_key,
        body: String::from_utf8_lossy(&body).into_owned(),
    };

    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    peak.fetch_max(now, Ordering::SeqCst);
    reqs.lock().unwrap().push(req.clone());
    let reply = handler(&req);
    thread::sleep(reply.delay);
    in_flight.fetch_sub(1, Ordering::SeqCst);

    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
}
