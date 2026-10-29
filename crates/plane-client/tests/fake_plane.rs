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

#[derive(Debug, Clone)]
pub struct FakeState {
    pub user: serde_json::Value,
    pub labels: Vec<serde_json::Value>,
    pub issues: HashMap<String, serde_json::Value>,
    pub comments: HashMap<String, Vec<serde_json::Value>>,
}

impl Default for FakeState {
    fn default() -> Self {
        FakeState {
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

pub fn handle_routes(req: &Req, state_lock: &Arc<Mutex<FakeState>>) -> Reply {
    let mut state = state_lock.lock().unwrap();
    let path = req.path_and_query.split('?').next().unwrap_or("");

    // 1. User route
    if path == "/api/v1/users/me/" {
        if req.api_key.as_deref() == Some("wrong-key") {
            return Reply {
                status: 401,
                body: "unauthorized".into(),
                delay: Duration::ZERO,
            };
        }
        return json(&state.user.to_string());
    }

    // 2. Labels routes
    if path.ends_with("/labels/") {
        if req.method == "GET" {
            let res = serde_json::json!({ "results": state.labels });
            return json(&res.to_string());
        }
        if req.method == "POST" {
            let body: serde_json::Value = match serde_json::from_str(&req.body) {
                Ok(v) => v,
                Err(_) => {
                    return Reply {
                        status: 400,
                        body: "bad json".into(),
                        delay: Duration::ZERO,
                    };
                }
            };
            let name = body["name"].as_str().unwrap_or("").to_string();
            let color = body["color"].as_str().unwrap_or("#7C3AED").to_string();
            let id = format!("lbl-{}", state.labels.len() + 1);
            let label = serde_json::json!({
                "id": id,
                "name": name,
                "color": color,
            });
            state.labels.push(label.clone());
            return json(&label.to_string());
        }
    }

    // 3. Comments routes
    if let Some(idx) = path.find("/issues/") {
        let rest = &path[idx + "/issues/".len()..];
        if let Some((issue_id, comment_part)) = rest.split_once("/comments/") {
            if !state.issues.contains_key(issue_id) {
                return Reply {
                    status: 404,
                    body: "{}".into(),
                    delay: Duration::ZERO,
                };
            }
            let cid = comment_part.trim_matches('/');
            if cid.is_empty() {
                // List or add comments
                if req.method == "GET" {
                    let list = state.comments.get(issue_id).cloned().unwrap_or_default();
                    return json(&serde_json::json!({ "results": list }).to_string());
                }
                if req.method == "POST" {
                    let body: serde_json::Value =
                        serde_json::from_str(&req.body).unwrap_or_default();
                    let html = body["comment_html"].as_str().unwrap_or("").to_string();
                    let entry = state.comments.entry(issue_id.to_string()).or_default();
                    let new_id = format!("c-{}", entry.len() + 1);
                    let comment = serde_json::json!({
                        "id": new_id,
                        "comment_html": html,
                        "created_at": format!("2026-10-28T12:00:{:02}Z", entry.len()),
                        "updated_at": format!("2026-10-28T12:00:{:02}Z", entry.len()),
                    });
                    entry.push(comment);
                    return json("{}");
                }
            } else {
                // Comment update
                if req.method == "PATCH" {
                    let entry = state.comments.entry(issue_id.to_string()).or_default();
                    if let Some(c) = entry.iter_mut().find(|c| c["id"] == cid) {
                        let body: serde_json::Value =
                            serde_json::from_str(&req.body).unwrap_or_default();
                        if let Some(html) = body["comment_html"].as_str() {
                            c["comment_html"] = serde_json::Value::String(html.to_string());
                            c["updated_at"] =
                                serde_json::Value::String("2026-10-28T12:30:00Z".to_string());
                        }
                        return json("{}");
                    }
                    return Reply {
                        status: 404,
                        body: "{}".into(),
                        delay: Duration::ZERO,
                    };
                }
            }
        }
    }

    // 4. Issue routes
    if let Some(idx) = path.find("/issues/") {
        let rest = &path[idx + "/issues/".len()..];
        let id = rest.trim_matches('/');
        if id.is_empty() {
            if req.method == "GET" {
                let list: Vec<serde_json::Value> = state.issues.values().cloned().collect();
                return json(
                    &serde_json::json!({ "results": list, "next_page_results": false, "next_cursor": "" })
                        .to_string(),
                );
            }
        } else if let Some(iss) = state.issues.get_mut(id) {
            if req.method == "GET" {
                return json(&iss.to_string());
            }
            if req.method == "PATCH" {
                let body: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
                if let Some(obj) = body.as_object() {
                    for (k, v) in obj {
                        iss[k] = v.clone();
                    }
                }
                return json(&iss.to_string());
            }
        } else {
            return Reply {
                status: 404,
                body: "{}".into(),
                delay: Duration::ZERO,
            };
        }
    }

    Reply {
        status: 404,
        body: "{}".into(),
        delay: Duration::ZERO,
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
