//! A std-only fake Ollama for tests: one accept loop on a thread, minimal HTTP/1.1,
//! deterministic bag-of-words embeddings and canned extractions. No tokio needed.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use serde_json::{json, Value};

#[derive(Default)]
struct State {
    extractions: Mutex<Vec<(String, Value)>>,
    calls: Mutex<Vec<String>>,
    fail_generate: AtomicUsize,
    down: AtomicBool,
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct FakeOllama {
    addr: SocketAddr,
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
}

impl FakeOllama {
    pub fn spawn(dim: usize) -> FakeOllama {
        let dim = dim.max(1);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake ollama");
        let addr = listener.local_addr().expect("fake ollama addr");
        let state = Arc::new(State::default());
        let st = Arc::clone(&state);
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if st.stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                if st.down.load(Ordering::SeqCst) {
                    drop(stream);
                    continue;
                }
                let _ = handle(&st, dim, stream);
            }
        });
        FakeOllama {
            addr,
            state,
            thread: Some(thread),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Generate calls whose `Section:` line contains `heading_substring` answer with `extraction`.
    pub fn set_extraction(&self, heading_substring: &str, extraction: Value) {
        let mut ex = lock(&self.state.extractions);
        ex.retain(|(k, _)| k != heading_substring);
        ex.push((heading_substring.to_string(), extraction));
    }

    pub fn fail_next_generate(&self, n: usize) {
        self.state.fail_generate.store(n, Ordering::SeqCst);
    }

    pub fn set_down(&self, down: bool) {
        self.state.down.store(down, Ordering::SeqCst);
    }

    pub fn calls(&self) -> Vec<String> {
        lock(&self.state.calls).clone()
    }
}

impl Drop for FakeOllama {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn handle(st: &State, dim: usize, stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let (method, path) = (
        words.next().unwrap_or("").to_string(),
        words.next().unwrap_or("").to_string(),
    );
    let mut len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    lock(&st.calls).push(path.clone());
    let resp = match (method.as_str(), path.as_str()) {
        ("GET", "/api/tags") => {
            json!({"models": [{"name": "qwen2.5:7b-instruct"}, {"name": "nomic-embed-text"}]})
        }
        ("POST", "/api/embed") => {
            let inputs: Vec<String> = match &req["input"] {
                Value::Array(a) => a
                    .iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect(),
                Value::String(s) => vec![s.clone()],
                _ => Vec::new(),
            };
            json!({"embeddings": inputs.iter().map(|t| embed(t, dim)).collect::<Vec<_>>()})
        }
        ("POST", "/api/generate") => {
            let failing = st
                .fail_generate
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
            if failing {
                json!({"response": "not json {"})
            } else {
                let prompt = req["prompt"].as_str().unwrap_or("");
                let section = prompt
                    .lines()
                    .find(|l| l.starts_with("Section:"))
                    .unwrap_or("");
                let found = lock(&st.extractions)
                    .iter()
                    .find(|(k, _)| section.contains(k.as_str()))
                    .map(|(_, v)| v.clone());
                let answer = found.unwrap_or_else(|| json!({"entities": [], "relations": []}));
                json!({"response": answer.to_string()})
            }
        }
        _ => json!({"error": "not found"}),
    };
    let body = resp.to_string();
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    out.flush()
}

/// Lowercase words hashed into `dim` buckets, counted, L2-normalised; empty text is a zero vector.
fn embed(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for w in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let h = blake3::hash(w.to_lowercase().as_bytes());
        let mut b = [0u8; 8];
        b.copy_from_slice(&h.as_bytes()[..8]);
        v[(u64::from_le_bytes(b) % dim as u64) as usize] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}
