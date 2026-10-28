//! A tiny in-process Plane: just enough HTTP for the client's tests, with request recording.
#![allow(dead_code)]

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

    pub fn recorded(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
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
