//! Minimal Ollama client over std::net — localhost HTTP only, so no TLS stack,
//! no async runtime and no C dependency end up in the binary.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Ollama {
    pub base: String,
    pub model: String,
    pub timeout: Duration,
}

impl Ollama {
    pub fn new(base: &str, model: &str, timeout: Duration) -> Self {
        Ollama {
            base: base.trim_end_matches('/').to_string(),
            model: model.to_string(),
            timeout,
        }
    }

    /// Models installed locally, e.g. `qwen2.5-coder:1.5b`.
    pub fn models(&self) -> Result<Vec<String>, String> {
        let body = self.request("GET", "/api/tags", None, Duration::from_secs(3))?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("invalid /api/tags payload: {e}"))?;
        Ok(v["models"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn generate(&self, prompt: &str) -> Result<String, String> {
        let payload = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "stream": false,
            "options": { "num_predict": 380, "temperature": 0.15 }
        });
        let body = self.request(
            "POST",
            "/api/generate",
            Some(&payload.to_string()),
            self.timeout,
        )?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("invalid /api/generate payload: {e}"))?;
        if let Some(err) = v["error"].as_str() {
            return Err(err.to_string());
        }
        Ok(v["response"].as_str().unwrap_or("").trim().to_string())
    }

    fn request(&self, method: &str, path: &str, body: Option<&str>, timeout: Duration) -> Result<String, String> {
        let (host, port) = host_port(&self.base)?;
        let addr = format!("{host}:{port}");
        let addrs: Vec<std::net::SocketAddr> = std::net::ToSocketAddrs::to_socket_addrs(&addr.as_str())
            .map_err(|e| format!("cannot resolve {addr}: {e}"))?
            .collect();
        let mut last = String::new();
        let mut connected = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, Duration::from_secs(3)) {
                Ok(s) => {
                    connected = Some(s);
                    break;
                }
                Err(e) => last = e.to_string(),
            }
        }
        let mut stream = connected
            .ok_or_else(|| format!("cannot reach ollama at {}: {last}", self.base))?;
        stream.set_read_timeout(Some(timeout)).ok();
        stream.set_write_timeout(Some(Duration::from_secs(10))).ok();

        let payload = body.unwrap_or("");
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| format!("write failed: {e}"))?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| format!("read failed (timeout {}s): {e}", timeout.as_secs()))?;
        parse_response(&raw)
    }
}

fn host_port(base: &str) -> Result<(String, u16), String> {
    let rest = base
        .strip_prefix("http://")
        .ok_or_else(|| "only http:// endpoints are supported".to_string())?;
    let rest = rest.split('/').next().unwrap_or(rest);
    match rest.rsplit_once(':') {
        Some((h, p)) => {
            let port = p.parse().map_err(|_| format!("bad port in {base}"))?;
            Ok((h.to_string(), port))
        }
        None => Ok((rest.to_string(), 80)),
    }
}

fn parse_response(raw: &[u8]) -> Result<String, String> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response".to_string())?;
    let head = String::from_utf8_lossy(&raw[..sep]).to_lowercase();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = &raw[sep + 4..];
    if head.contains("transfer-encoding: chunked") {
        return dechunk(body);
    }
    if !(200..300).contains(&status) {
        return Err(format!("ollama answered HTTP {status}: {}", String::from_utf8_lossy(body)));
    }
    Ok(String::from_utf8_lossy(body).to_string())
}

fn dechunk(mut body: &[u8]) -> Result<String, String> {
    let mut out = Vec::new();
    loop {
        let Some(nl) = body.windows(2).position(|w| w == b"\r\n") else { break };
        let size = usize::from_str_radix(
            String::from_utf8_lossy(&body[..nl]).trim(),
            16,
        )
        .map_err(|_| "bad chunk size".to_string())?;
        body = &body[nl + 2..];
        if size == 0 {
            break;
        }
        if body.len() < size {
            break;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[(size + 2).min(body.len())..];
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// One finding, one short answer: verdict, plan, risk. No code dumps.
pub fn prompt_for(name: &str, lang: &str, category: &str, why: &str, source: &str) -> String {
    format!(
        "You are reviewing a static-analysis finding in a {lang} project. \
Reply in plain text, no markdown fences, in at most 12 lines, with exactly this layout:\n\
VERDICT: <safe to delete | refactor | false positive | needs review>\n\
WHY: <one sentence>\n\
PLAN: <2-4 numbered steps>\n\
RISK: <what could break, one sentence>\n\
Be specific to the code below. Do not rewrite the function.\n\n\
Finding: {category} on `{name}`.\nAnalyzer says: {why}\n\n```\n{source}\n```\n"
    )
}
