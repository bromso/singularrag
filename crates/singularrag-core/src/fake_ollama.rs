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
    steps: Mutex<Vec<(String, Value)>>,
    calls: Mutex<Vec<String>>,
    fail_generate: AtomicUsize,
    corrupt_embed: AtomicUsize,
    drop_next: AtomicUsize,
    accepted: AtomicUsize,
    down: AtomicBool,
    /// `set_down_after`: 0 = inactive; `k + 1` = `k` more generate answers, so 1 = generate is down.
    generate_down_after: AtomicUsize,
    generate_delay_ms: AtomicUsize,
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
                st.accepted.fetch_add(1, Ordering::SeqCst);
                if st.down.load(Ordering::SeqCst) || take_one(&st.drop_next) {
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

    /// Steps-prompt generate calls whose `Section:` line contains `heading_substring` answer with `answer`.
    pub fn set_steps(&self, heading_substring: &str, answer: Value) {
        let mut st = lock(&self.state.steps);
        st.retain(|(k, _)| k != heading_substring);
        st.push((heading_substring.to_string(), answer));
    }

    pub fn fail_next_generate(&self, n: usize) {
        self.state.fail_generate.store(n, Ordering::SeqCst);
    }

    /// The next `n` embed responses carry a `null` inside the first vector.
    pub fn corrupt_next_embed(&self, n: usize) {
        self.state.corrupt_embed.store(n, Ordering::SeqCst);
    }

    /// The next `n` connections are accepted and closed without reading the request.
    pub fn drop_next_n_connections(&self, n: usize) {
        self.state.drop_next.store(n, Ordering::SeqCst);
    }

    /// Connections accepted so far, dropped ones included.
    pub fn accepted(&self) -> usize {
        self.state.accepted.load(Ordering::SeqCst)
    }

    /// Every generate call sleeps this long before answering.
    pub fn set_generate_delay(&self, delay: std::time::Duration) {
        self.state
            .generate_delay_ms
            .store(delay.as_millis() as usize, Ordering::SeqCst);
    }

    /// `false` also clears `set_down_after`.
    pub fn set_down(&self, down: bool) {
        self.state.down.store(down, Ordering::SeqCst);
        if !down {
            self.state.generate_down_after.store(0, Ordering::SeqCst);
        }
    }

    /// After `n` more answered `/api/generate` requests (either prompt kind), generate
    /// requests are read and closed without an answer, as a server gone down would. Only
    /// generate goes down, so the embed call that follows an answered entity prompt still
    /// works. `set_down(false)` clears it.
    pub fn set_down_after(&self, n: usize) {
        self.state
            .generate_down_after
            .store(n.saturating_add(1), Ordering::SeqCst);
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

/// Decrement a "next n" counter; true when it was above zero.
fn take_one(n: &AtomicUsize) -> bool {
    n.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
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
    if !(method == "POST" && path == "/api/generate") {
        lock(&st.calls).push(path.clone());
    }
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
            let mut vectors: Vec<Value> = inputs.iter().map(|t| json!(embed(t, dim))).collect();
            if take_one(&st.corrupt_embed) {
                if let Some(v) = vectors.first_mut() {
                    v[0] = Value::Null;
                }
            }
            json!({ "embeddings": vectors })
        }
        ("POST", "/api/generate") => {
            if st.generate_down_after.load(Ordering::SeqCst) == 1 {
                return Ok(());
            }
            let _ = st
                .generate_down_after
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    (n > 1).then(|| n - 1)
                });
            let delay = st.generate_delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay as u64));
            }
            let prompt = req["prompt"].as_str().unwrap_or("");
            let is_steps = prompt.starts_with(crate::models::STEPS_PROMPT_FIRST_LINE);
            lock(&st.calls).push(if is_steps {
                "/api/generate:steps".into()
            } else {
                "/api/generate".into()
            });
            if take_one(&st.fail_generate) {
                json!({"response": "not json {"})
            } else {
                let section = prompt
                    .lines()
                    .find(|l| l.starts_with("Section:"))
                    .unwrap_or("");
                if is_steps {
                    let found = lock(&st.steps)
                        .iter()
                        .find(|(k, _)| section.contains(k.as_str()))
                        .map(|(_, v)| v.clone());
                    let answer = found.unwrap_or_else(|| json!({"process": "", "steps": []}));
                    json!({"response": answer.to_string()})
                } else {
                    let found = lock(&st.extractions)
                        .iter()
                        .find(|(k, _)| section.contains(k.as_str()))
                        .map(|(_, v)| v.clone());
                    let answer = found.unwrap_or_else(|| json!({"entities": [], "relations": []}));
                    json!({"response": answer.to_string()})
                }
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
