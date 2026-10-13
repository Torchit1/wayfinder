//! Minimal HTTP/1.1: one request per connection, Content-Length bodies only.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub auth: String,
    pub body: String,
    /// IPv4 peer as a u32 (0 for anything else)
    pub peer: u32,
}

pub struct Response {
    pub status: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(&'static str, String)>,
}

impl Response {
    pub fn json(status: u16, body: String) -> Self {
        Response { status, ctype: "application/json", body: body.into_bytes(), headers: Vec::new() }
    }
}

/// Listen on `ip:port` (0 = all interfaces). Bind first so the caller learns about a busy port.
pub fn bind(ip: u32, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::from(ip), port))
}

pub fn serve(listener: TcpListener, handler: impl Fn(Request) -> Response + Send + Sync + 'static) -> std::io::Result<()> {
    let handler = std::sync::Arc::new(handler);
    for stream in listener.incoming().flatten() {
        let h = handler.clone();
        std::thread::spawn(move || {
            let _ = handle(stream, &*h);
        });
    }
    Ok(())
}

fn handle(mut stream: TcpStream, handler: &dyn Fn(Request) -> Response) -> std::io::Result<()> {
    let peer = match stream.peer_addr()?.ip() {
        IpAddr::V4(a) => u32::from(a),
        IpAddr::V6(_) => 0,
    };
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut r = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (mut len, mut auth) = (0usize, String::new());
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0).min(1 << 20),
                "authorization" => auth = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let req = Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        auth,
        body: String::from_utf8_lossy(&body).into_owned(),
        peer,
    };
    let resp = handler(req);
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status,
        resp.ctype,
        resp.body.len()
    );
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&resp.body)
}

/// "a.b.c.d" -> u32 (None if malformed). Hand-rolled to keep std's address parser out.
pub fn ipv4(s: &str) -> Option<u32> {
    let mut parts = s.trim().split('.');
    let mut v = 0u32;
    for _ in 0..4 {
        let n = crate::json::int(parts.next()?)?;
        if !(0..=255).contains(&n) {
            return None;
        }
        v = v << 8 | n as u32;
    }
    parts.next().is_none().then_some(v)
}

/// Query-string value (percent-decoded, '+' as space).
pub fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| decode(v))
    })
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn encode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect()
}

/// Blocking client request to 127.0.0.1. Returns (status, headers, body).
pub fn request(
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    body: &str,
    timeout_s: u64,
) -> std::io::Result<(u16, Vec<(String, String)>, Vec<u8>)> {
    let mut s = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(timeout_s)))?;
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let status = line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let mut body = Vec::new();
    r.read_to_end(&mut body)?;
    Ok((status, headers, body))
}
