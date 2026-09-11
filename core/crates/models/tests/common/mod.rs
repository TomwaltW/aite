//! R5 私有的测试替身：一个最小 HTTP/1.1 假服务。
//!
//! 为什么不用 wiremock：workspace 的依赖是冻结的（spec §7 纪律 4），加库要停下报告。
//! 这里要的只有三件事 —— 收下请求、记下它、回一份 canned JSON，自己起个
//! `tokio::net::TcpListener` 就够了，也顺带把「请求真的发出去了」这件事验到位
//! （Python 版那几条只能查 SDK 客户端上的 base_url / api_key，拦不到实际流量）。
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Value,
}

impl RecordedRequest {
    /// 请求体里的某个顶层键。
    pub fn field(&self, key: &str) -> Option<&Value> {
        self.body.get(key)
    }

    pub fn has(&self, key: &str) -> bool {
        self.body.get(key).is_some()
    }
}

struct ServerState {
    requests: Vec<RecordedRequest>,
    responses: Vec<(u16, String)>,
    next: usize,
}

pub struct FakeServer {
    pub base_url: String,
    state: Arc<Mutex<ServerState>>,
}

impl FakeServer {
    /// 每次请求回一条；用完之后重复最后一条。
    pub async fn start(responses: Vec<Value>) -> Self {
        let coded: Vec<(u16, String)> = responses
            .into_iter()
            .map(|v| (200u16, v.to_string()))
            .collect();
        Self::start_raw(coded).await
    }

    pub async fn start_raw(responses: Vec<(u16, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(ServerState {
            requests: Vec::new(),
            responses,
            next: 0,
        }));
        let task_state = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let state = task_state.clone();
                tokio::spawn(async move {
                    let Some((req, body_bytes)) = read_request(&mut sock).await else {
                        return;
                    };
                    let body: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
                    let (code, payload) = {
                        let mut st = state.lock().expect("server");
                        st.requests.push(RecordedRequest { body, ..req });
                        let idx = st.next.min(st.responses.len().saturating_sub(1));
                        st.next += 1;
                        st.responses
                            .get(idx)
                            .cloned()
                            .unwrap_or((200, "{}".to_string()))
                    };
                    let reason = if code == 200 { "OK" } else { "Error" };
                    let head = format!(
                        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(payload.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        Self {
            base_url: format!("http://{addr}/v1"),
            state,
        }
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().expect("server").requests.clone()
    }

    pub fn request(&self, i: usize) -> RecordedRequest {
        self.requests()
            .into_iter()
            .nth(i)
            .expect("假服务没收到这次请求")
    }

    pub fn request_count(&self) -> usize {
        self.state.lock().expect("server").requests.len()
    }
}

/// 读到 `\r\n\r\n` 为止拿请求行与 header，再按 Content-Length 把 body 读完。
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<(RecordedRequest, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let want: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < want {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(want);

    Some((
        RecordedRequest {
            method,
            path,
            headers,
            body: Value::Null,
        },
        body,
    ))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
