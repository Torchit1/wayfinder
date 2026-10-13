//! pointer: lets an AI agent point at things on a Windows screen instead of clicking them.
//! One process (`pointer serve`) owns the overlay, the UI index and a small token-protected HTTP
//! API; every other subcommand is a thin client of that API.

mod http;
mod index;
mod json;
mod overlay;
mod shot;
mod win;

use http::{Request, Response};
use index::Index;
use json::q;
use overlay::{Mode, Shared};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 8790;
const USAGE: &str = "pointer: point at things on screen for a person to click (Windows)

  pointer serve [--port N] [--lan --allow IP,IP]   run the overlay + API (localhost only unless --lan)
  pointer at \"name\" [\"label\"]       arrow on a control found by name (no screenshot); learned route if hidden
  pointer find \"name\"               list visible controls whose name contains this
  pointer shot [file.png] [--width N]  screenshot of the whole desktop with the mouse marked
  pointer point X Y [\"label\"] [--raw] arrow at X,Y in the last shot's pixels (--raw: screen pixels)
  pointer wait [seconds]            block until the person clicks inside the ring (exit 0) or timeout (1)
  pointer busy [\"message\"]          'working out the next step' badge
  pointer clip \"text\"               put text on the clipboard so they can right-click > Paste
  pointer off | quit | health

Env: POINTER_PORT (default 8790). Token: token.txt next to pointer.exe";

fn main() {
    unsafe {
        use windows::Win32::UI::HiDpi::*;
        _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        Some(cmd) if cmd != "help" && cmd != "--help" && cmd != "-h" => client(cmd, &args[1..]),
        _ => {
            println!("{USAGE}");
            0
        }
    };
    std::process::exit(code);
}

fn token() -> String {
    let path = win::data_dir().join("token.txt");
    if let Ok(t) = std::fs::read_to_string(&path) {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    let mut b = [0u8; 24];
    unsafe {
        use windows::Win32::Security::Cryptography::*;
        _ = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let _ = std::fs::write(&path, &t);
    t
}

fn port() -> u16 {
    std::env::var("POINTER_PORT").ok().and_then(|p| json::int(&p)).map_or(DEFAULT_PORT, |p| p as u16)
}

// ---------------------------------------------------------------- server

struct Server {
    shared: &'static Shared,
    index: &'static Index,
    token: String,
    allow: Vec<String>,
    /// last /shot mapping: (scale, origin) so /point can take shot-pixel coordinates
    shot_map: Mutex<Option<(f64, (i32, i32))>>,
}

fn serve(args: &[String]) -> i32 {
    let mut port = port();
    let mut lan = false;
    let mut allow = vec!["127.0.0.1".to_string()];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => port = it.next().and_then(|p| json::int(p)).map_or(port, |p| p as u16),
            "--lan" => lan = true,
            "--allow" => allow.extend(it.next().map(|s| s.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).unwrap_or_default()),
            _ => {}
        }
    }
    // console exe so CLI output works everywhere; the long-running server lets its console go
    unsafe { _ = windows::Win32::System::Console::FreeConsole() };
    let index = Index::start();
    let shared: &'static Shared = Box::leak(Box::new(Shared::new()));
    let srv: &'static Server =
        Box::leak(Box::new(Server { shared, index, token: token(), allow, shot_map: Mutex::new(None) }));
    let addr = format!("{}:{port}", if lan { "0.0.0.0" } else { "127.0.0.1" });
    let bound = std::net::TcpListener::bind(&addr).map(drop);
    if let Err(e) = bound {
        eprintln!("pointer: can't listen on {addr}: {e} (already running?)");
        return 1;
    }
    std::thread::spawn(move || {
        let _ = http::serve(&addr, move |r| srv.handle(r));
    });
    overlay::run(shared, index);
    0
}

impl Server {
    fn handle(&self, r: Request) -> Response {
        if !self.allow.iter().any(|a| *a == r.peer.to_string()) {
            return Response::json(403, r#"{"error":"forbidden"}"#.into());
        }
        if r.auth != format!("Bearer {}", self.token) {
            return Response::json(401, r#"{"error":"bad token"}"#.into());
        }
        let body = json::parse_object(&r.body).unwrap_or_default();
        let s = |k: &str| body.get(k).and_then(|v| v.str()).unwrap_or("").to_string();
        let n = |k: &str| body.get(k).and_then(|v| v.num());
        let ok = || Response::json(200, r#"{"ok":true}"#.into());
        match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/health") => {
                let (w, i, l) = self.index.stats();
                Response::json(200, format!(r#"{{"ok":true,"windows":{w},"items":{i},"learned":{l}}}"#))
            }
            ("GET", "/find") => {
                let qy = http::query_param(&r.query, "q").unwrap_or_default();
                let m = self.index.lookup(&qy, 10);
                let route = if m.is_empty() { self.route_json(&qy) } else { "null".into() };
                Response::json(200, format!(r#"{{"matches":[{}],"route":{route}}}"#, matches_json(&m)))
            }
            ("POST", "/at") => self.at(&s("q"), &s("label")),
            ("POST", "/point") => {
                let (Some(x), Some(y)) = (n("x"), n("y")) else {
                    return Response::json(400, r#"{"error":"need x, y"}"#.into());
                };
                let (x, y) = if body.get("raw").is_some_and(|v| v.truthy()) {
                    (x as i32, y as i32)
                } else {
                    let Some((scale, (ox, oy))) = *self.shot_map.lock().unwrap_or_else(|e| e.into_inner()) else {
                        return Response::json(409, r#"{"error":"take a shot first, or pass raw:true"}"#.into());
                    };
                    ((x as f64 * scale) as i32 + ox, (y as f64 * scale) as i32 + oy)
                };
                self.shared.set_target(x, y, &s("label"));
                ok()
            }
            ("GET", "/shot") => {
                let w = http::query_param(&r.query, "width").and_then(|w| json::int(&w)).unwrap_or(2560) as u32;
                let Some(sh) = shot::take(w) else {
                    return Response::json(503, r#"{"error":"screen grab failed: an admin (UAC) prompt is probably on screen"}"#.into());
                };
                *self.shot_map.lock().unwrap_or_else(|e| e.into_inner()) = Some((sh.scale, sh.origin));
                let (vx, _, vw, _) = win::virtual_screen();
                let mid = ((sh.monitor.0 + sh.monitor.2) as f64 * sh.scale / 2.0) as i32 + vx;
                let side = if sh.monitor.2 - sh.monitor.0 >= sh.size.0 as i32 - 1 {
                    "only"
                } else if mid < vx + vw / 2 {
                    "left"
                } else {
                    "right"
                };
                let m = sh.monitor;
                Response {
                    status: 200,
                    ctype: "image/png",
                    headers: vec![
                        ("X-Cursor", format!("{},{}", sh.cursor.0, sh.cursor.1)),
                        ("X-Cursor-Monitor", side.into()),
                        ("X-Monitor-Rect", format!("{},{},{},{}", m.0, m.1, m.2, m.3)),
                        ("X-Shot", {
                            let milli = (sh.scale * 1000.0).round() as u64;
                            format!("{}x{} scale={}.{:03}", sh.size.0, sh.size.1, milli / 1000, milli % 1000)
                        }),
                    ],
                    body: sh.png,
                }
            }
            ("GET", "/wait") => {
                let t = http::query_param(&r.query, "timeout").and_then(|t| json::int(&t)).unwrap_or(600).clamp(0, 900);
                match self.shared.wait_hit(Duration::from_secs(t as u64)) {
                    Some((x, y)) => Response::json(200, format!(r#"{{"hit":true,"x":{x},"y":{y}}}"#)),
                    None => Response::json(200, r#"{"hit":false}"#.into()),
                }
            }
            ("POST", "/busy") => {
                let msg = s("msg");
                let msg = if msg.is_empty() { overlay::BUSY_MSG.to_string() } else { msg };
                *self.shared.mode.lock().unwrap_or_else(|e| e.into_inner()) = Mode::Busy { msg, since: Instant::now() };
                ok()
            }
            ("POST", "/clip") => {
                if win::set_clipboard(&s("text")) {
                    ok()
                } else {
                    Response::json(503, r#"{"error":"clipboard busy"}"#.into())
                }
            }
            ("POST", "/off") => {
                *self.shared.mode.lock().unwrap_or_else(|e| e.into_inner()) = Mode::Hidden;
                ok()
            }
            ("POST", "/quit") => {
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(100));
                    std::process::exit(0)
                });
                ok()
            }
            _ => Response::json(404, r#"{"error":"not found"}"#.into()),
        }
    }

    fn at(&self, query: &str, label: &str) -> Response {
        if let Some(m) = self.index.lookup(query, 1).into_iter().next() {
            let label = if label.is_empty() { format!("Click {}", m.name) } else { label.into() };
            self.shared.set_target(m.center.0, m.center.1, &label);
            return Response::json(200, format!(r#"{{"pointed":{},"step":1,"of":1}}"#, match_json(&m)));
        }
        let Some(steps) = self.index.route(query, 3) else {
            return Response::json(404, format!(r#"{{"error":{}}}"#, q(&format!("'{query}' isn't on screen and no learned route to it"))));
        };
        let Some(first) = self.index.lookup(&steps[0].0, 1).into_iter().next() else {
            return Response::json(404, r#"{"error":"route start not on screen"}"#.into());
        };
        let verb = if steps[0].1 == "right" { "RIGHT-click" } else { "Click" };
        let label = if label.is_empty() {
            format!("{verb} {} (to reach {})", first.name, steps[steps.len() - 1].0)
        } else {
            label.into()
        };
        self.shared.set_target(first.center.0, first.center.1, &label);
        Response::json(
            200,
            format!(
                r#"{{"pointed":{},"button":{},"route":{},"step":1,"of":{}}}"#,
                match_json(&first),
                q(&steps[0].1),
                self.route_json(query),
                steps.len()
            ),
        )
    }

    fn route_json(&self, query: &str) -> String {
        match self.index.route(query, 3) {
            None => "null".into(),
            Some(steps) => format!(
                "[{}]",
                steps.iter().map(|(n, b)| format!(r#"{{"name":{},"button":{}}}"#, q(n), q(b))).collect::<Vec<_>>().join(",")
            ),
        }
    }
}

fn match_json(m: &index::Match) -> String {
    let r = m.rect;
    format!(
        r#"{{"name":{},"type":{},"rect":[{},{},{},{}],"center":[{},{}],"window":{},"score":{}}}"#,
        q(&m.name),
        q(&m.ctype),
        r.0,
        r.1,
        r.2,
        r.3,
        m.center.0,
        m.center.1,
        q(&m.window),
        m.score
    )
}

fn matches_json(ms: &[index::Match]) -> String {
    ms.iter().map(match_json).collect::<Vec<_>>().join(",")
}

// ---------------------------------------------------------------- client

fn client(cmd: &str, args: &[String]) -> i32 {
    let tok = token();
    let call = |method: &str, path: &str, body: &str, timeout: u64| http::request(port(), method, path, &tok, body, timeout);
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    let res = match cmd {
        "health" => call("GET", "/health", "", 5),
        "find" => call("GET", &format!("/find?q={}", http::encode(arg(0))), "", 5),
        "at" => call("POST", "/at", &format!(r#"{{"q":{},"label":{}}}"#, q(arg(0)), q(arg(1))), 5),
        "wait" => {
            let t = json::int(arg(0)).unwrap_or(600).max(0) as u64;
            call("GET", &format!("/wait?timeout={t}"), "", t + 10)
        }
        "busy" => call("POST", "/busy", &format!(r#"{{"msg":{}}}"#, q(arg(0))), 5),
        "clip" => call("POST", "/clip", &format!(r#"{{"text":{}}}"#, q(arg(0))), 5),
        "off" => call("POST", "/off", "", 5),
        "quit" => call("POST", "/quit", "", 5),
        "shot" => {
            let mut file = "shot.png".to_string();
            let mut width = 2560;
            let mut i = 0;
            while i < args.len() {
                if args[i] == "--width" {
                    width = args.get(i + 1).and_then(|w| json::int(w)).unwrap_or(width);
                    i += 1;
                } else {
                    file = args[i].clone();
                }
                i += 1;
            }
            return match call("GET", &format!("/shot?width={width}"), "", 30) {
                Ok((200, h, body)) => {
                    if let Err(e) = std::fs::write(&file, body) {
                        eprintln!("can't write {file}: {e}");
                        return 1;
                    }
                    let hv = |k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("?");
                    println!("{file} {}", hv("x-shot"));
                    println!("mouse at {} on the {} monitor (shot rect {})", hv("x-cursor"), hv("x-cursor-monitor"), hv("x-monitor-rect"));
                    0
                }
                other => report(other),
            };
        }
        "point" => {
            let (Some(x), Some(y)) = (json::int(arg(0)), json::int(arg(1))) else {
                eprintln!("usage: pointer point X Y [label] [--raw]");
                return 2;
            };
            let raw = args.iter().any(|a| a == "--raw");
            let label = args.get(2).filter(|a| *a != "--raw").map(String::as_str).unwrap_or("");
            call("POST", "/point", &format!(r#"{{"x":{x},"y":{y},"label":{},"raw":{raw}}}"#, q(label)), 5)
        }
        _ => {
            eprintln!("unknown command '{cmd}'\n\n{USAGE}");
            return 2;
        }
    };
    let code = report(res);
    if cmd == "wait" && code == 0 {
        return 0;
    }
    code
}

fn report(res: std::io::Result<(u16, Vec<(String, String)>, Vec<u8>)>) -> i32 {
    match res {
        Ok((status, _, body)) => {
            let text = String::from_utf8_lossy(&body);
            println!("{text}");
            if status != 200 || text.contains(r#""hit":false"#) { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("pointer server not reachable on 127.0.0.1:{} ({e}). Start it: pointer serve", port());
            3
        }
    }
}
