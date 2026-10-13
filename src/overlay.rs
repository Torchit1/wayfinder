//! The arrow / ring / label and the "working on the next step" badge. A small click-through,
//! always-on-top, colour-keyed window that is resized to fit what it draws and hidden when idle.

use crate::index::Index;
use crate::win::{self, Rect};
use std::cell::RefCell;
use std::f64::consts::SQRT_2;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

const KEY: u32 = 0x010101; // transparent colour key
const RED: u32 = 0x2d2dff; // COLORREF is 0x00BBGGRR
const GREEN: u32 = 0x6aff2d;
const AMBER: u32 = 0x2db0ff;
const WHITE: u32 = 0xffffff;
const PANEL: u32 = 0x1e1e1e;
const HIT_R: f64 = 45.0;
const FLASH: Duration = Duration::from_millis(500);
const BUSY_STALE: Duration = Duration::from_secs(120);
pub const BUSY_MSG: &str = "Claude is working out the next step";

pub enum Mode {
    Hidden,
    Target { x: i32, y: i32, label: String, id: u64, done: Option<Instant> },
    Busy { msg: String, since: Instant },
}

pub struct Hit {
    pub id: u64,
    pub x: i32,
    pub y: i32,
}

pub struct Shared {
    pub mode: Mutex<Mode>,
    pub hit: Mutex<Option<Hit>>,
    pub hit_cv: Condvar,
    pub next_id: Mutex<u64>,
}

impl Shared {
    pub fn new() -> Self {
        Shared { mode: Mutex::new(Mode::Hidden), hit: Mutex::new(None), hit_cv: Condvar::new(), next_id: Mutex::new(0) }
    }

    pub fn set_target(&self, x: i32, y: i32, label: &str) -> u64 {
        let id = {
            let mut n = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            *n += 1;
            *n
        };
        *self.hit.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = Mode::Target { x, y, label: label.into(), id, done: None };
        id
    }

    /// Block until the current target is clicked, or timeout.
    pub fn wait_hit(&self, timeout: Duration) -> Option<(i32, i32)> {
        let id = match &*self.mode.lock().unwrap_or_else(|e| e.into_inner()) {
            Mode::Target { id, .. } => *id,
            _ => *self.next_id.lock().unwrap_or_else(|e| e.into_inner()),
        };
        let g = self.hit.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self.hit_cv.wait_timeout_while(g, timeout, |h| !h.as_ref().is_some_and(|h| h.id == id)).unwrap_or_else(|e| e.into_inner());
        g.as_ref().filter(|h| h.id == id).map(|h| (h.x, h.y))
    }
}

/// What to paint, in window-relative pixels.
enum Scene {
    Arrow { tail: (i32, i32), base: (i32, i32), tip: (i32, i32), ring: (i32, i32, i32), ring_col: u32, label: Option<(RECT, String)> },
    Badge { panel: RECT, text: RECT, msg: String, col: u32, dot: Option<RECT> },
}

struct Frame {
    origin: (i32, i32),
    size: (i32, i32),
    scene: Scene,
}

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
    static FONTS: RefCell<(HFONT, HFONT)> = const { RefCell::new((HFONT(std::ptr::null_mut()), HFONT(std::ptr::null_mut()))) };
}

pub fn run(shared: &'static Shared, index: &'static Index) {
    unsafe {
        let inst = GetModuleHandleW(None).unwrap_or_default();
        let cls = win::wide("WayfinderOverlay");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: inst.into(),
            lpszClassName: PCWSTR(cls.as_ptr()),
            ..Default::default()
        });
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            PCWSTR(cls.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(inst.into()),
            None,
        ) else {
            return;
        };
        _ = SetLayeredWindowAttributes(hwnd, COLORREF(KEY), 0, LWA_COLORKEY);
        let font = |h: i32| CreateFontW(h, 0, 0, 0, 700, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, 0, PCWSTR(win::wide("Segoe UI").as_ptr()));
        FONTS.with(|f| *f.borrow_mut() = (font(-26), font(-22)));
        SetTimer(Some(hwnd), 1, 15, None);
        let mut st = Tick { was: [false; 2], shown: false };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER {
                st.tick(hwnd, shared, index);
            }
            DispatchMessageW(&msg);
        }
    }
}

struct Tick {
    was: [bool; 2],
    shown: bool,
}

impl Tick {
    fn tick(&mut self, hwnd: HWND, shared: &Shared, index: &Index) {
        // clicks: feed the route learner, and detect a hit on the target ring
        for (i, (vk, button)) in [(0x01, "left"), (0x02, "right")].into_iter().enumerate() {
            let down = unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0;
            if down && !self.was[i] {
                let (x, y) = win::cursor();
                index.on_click(x, y, button);
                let mut m = shared.mode.lock().unwrap_or_else(|e| e.into_inner());
                if let Mode::Target { x: tx, y: ty, id, done, .. } = &mut *m {
                    if done.is_none() && (((x - *tx).pow(2) + (y - *ty).pow(2)) as f64).sqrt() <= HIT_R {
                        *done = Some(Instant::now());
                        *shared.hit.lock().unwrap_or_else(|e| e.into_inner()) = Some(Hit { id: *id, x, y });
                        shared.hit_cv.notify_all();
                    }
                }
            }
            self.was[i] = down;
        }
        // after the green flash, show the badge until the next instruction arrives
        let frame = {
            let mut m = shared.mode.lock().unwrap_or_else(|e| e.into_inner());
            if let Mode::Target { done: Some(t), .. } = &*m {
                if t.elapsed() > FLASH {
                    *m = Mode::Busy { msg: BUSY_MSG.into(), since: Instant::now() };
                }
            }
            layout(&m)
        };
        unsafe {
            match frame {
                None => {
                    if self.shown {
                        // hide outright: an emptied colour-keyed window can leave unrepainted slivers
                        _ = ShowWindow(hwnd, SW_HIDE);
                        self.shown = false;
                    }
                    FRAME.with(|f| *f.borrow_mut() = None);
                }
                Some(fr) => {
                    _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), fr.origin.0, fr.origin.1, fr.size.0, fr.size.1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    self.shown = true;
                    FRAME.with(|f| *f.borrow_mut() = Some(fr));
                    win::invalidate(hwnd);
                }
            }
        }
    }
}

fn layout(m: &Mode) -> Option<Frame> {
    match m {
        Mode::Hidden => None,
        Mode::Target { x, y, label, done, .. } => Some(arrow_layout(*x, *y, label, done.is_some())),
        Mode::Busy { msg, since } => Some(badge_layout(msg, since.elapsed() > BUSY_STALE)),
    }
}

fn measure(text: &str, font_idx: usize, width: i32) -> (i32, i32) {
    unsafe {
        let dc = GetDC(None);
        let f = FONTS.with(|f| if font_idx == 0 { f.borrow().0 } else { f.borrow().1 });
        let old = SelectObject(dc, f.into());
        let mut w = win::wide(text);
        w.pop();
        let mut r = RECT { left: 0, top: 0, right: width, bottom: 0 };
        DrawTextW(dc, &mut w, &mut r, DT_CALCRECT | DT_WORDBREAK);
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        (r.right, r.bottom)
    }
}

fn arrow_layout(tx: i32, ty: i32, label: &str, done: bool) -> Frame {
    let (_, (ml, mt, mr, mb)) = win::monitor_at(tx, ty);
    // tail comes from whichever side of the monitor has more room
    let dx = if tx - ml > mr - tx { -1.0 } else { 1.0 };
    let dy = if ty - mt > mb - ty { -1.0 } else { 1.0 };
    let at = |d: f64| ((tx as f64 + dx * d / SQRT_2) as i32, (ty as f64 + dy * d / SQRT_2) as i32);
    let (tail, tip, base) = (at(190.0), at(34.0), at(80.0));
    let t = Instant::now().duration_since(*START.get_or_init(Instant::now)).as_secs_f64();
    let rad = if done { 34 } else { 30 + (5.0 * (1.0 + (t * 3.75).sin())) as i32 };
    let mut bounds = (tx - 50, ty - 50, tx + 50, ty + 50);
    let mut grow = |r: Rect| bounds = (bounds.0.min(r.0), bounds.1.min(r.1), bounds.2.max(r.2), bounds.3.max(r.3));
    grow((tail.0 - 20, tail.1 - 20, tail.0 + 20, tail.1 + 20));
    let label = (!label.is_empty()).then(|| {
        let (w, h) = measure(label, 0, 520);
        let x0 = if dx < 0.0 { tail.0 - w } else { tail.0 };
        let y0 = if dy < 0.0 { tail.1 - h } else { tail.1 };
        // keep the panel (text + 12/8 padding + 10 margin) inside the monitor's work area
        let sx = (ml + 22 - x0).max(0) - (x0 + w + 22 - mr).max(0);
        let sy = (mt + 18 - y0).max(0) - (y0 + h + 18 - mb).max(0);
        let r = RECT { left: x0 + sx, top: y0 + sy, right: x0 + sx + w, bottom: y0 + sy + h };
        grow((r.left - 16, r.top - 12, r.right + 16, r.bottom + 12));
        (r, label.to_string())
    });
    let o = (bounds.0, bounds.1);
    let rel = |p: (i32, i32)| (p.0 - o.0, p.1 - o.1);
    Frame {
        origin: o,
        size: (bounds.2 - bounds.0, bounds.3 - bounds.1),
        scene: Scene::Arrow {
            tail: rel(tail),
            base: rel(base),
            tip: rel(tip),
            ring: (tx - o.0, ty - o.1, rad),
            ring_col: if done { GREEN } else { RED },
            label: label.map(|(r, s)| (RECT { left: r.left - o.0, top: r.top - o.1, right: r.right - o.0, bottom: r.bottom - o.1 }, s)),
        },
    }
}

fn badge_layout(msg: &str, stale: bool) -> Frame {
    let (cx, cy) = win::cursor();
    let (_, (ml, _, mr, mb)) = win::monitor_at(cx, cy);
    let msg = if stale { "Waiting on Claude: check the chat" } else { msg };
    let (w, h) = measure(&format!("{msg}..."), 1, 2000);
    let (pw, ph) = (w + 46 + 16, h + 20);
    let origin = ((ml + mr) / 2 - pw / 2, mb - 30 - ph);
    let t = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    let dots = ".".repeat(1 + (t * 2.5) as usize % 3);
    let dot_on = (t * 5.0).sin() > -0.6;
    Frame {
        origin,
        size: (pw, ph),
        scene: Scene::Badge {
            panel: RECT { left: 0, top: 0, right: pw, bottom: ph },
            text: RECT { left: 46, top: 10, right: pw - 8, bottom: ph - 10 },
            msg: format!("{msg}{dots}"),
            col: if stale { AMBER } else { GREEN },
            dot: dot_on.then_some(RECT { left: 14, top: ph / 2 - 7, right: 28, bottom: ph / 2 + 7 }),
        },
    }
}

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_PAINT {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            FRAME.with(|f| {
                if let Some(fr) = &*f.borrow() {
                    paint(dc, fr);
                }
            });
            _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        if msg == WM_ERASEBKGND {
            return LRESULT(1);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

unsafe fn pen(col: u32, w: u32) -> HPEN {
    let lb = LOGBRUSH { lbStyle: BS_SOLID, lbColor: COLORREF(col), lbHatch: 0 };
    unsafe { ExtCreatePen(PS_GEOMETRIC | PS_SOLID | PS_ENDCAP_ROUND | PS_JOIN_ROUND, w, &lb, None) }
}

/// Draw with `pen`/`brush` selected, then free them.
unsafe fn with(dc: HDC, p: HPEN, b: HGDIOBJ, f: impl FnOnce()) {
    unsafe {
        let op = SelectObject(dc, p.into());
        let ob = SelectObject(dc, b);
        f();
        SelectObject(dc, op);
        SelectObject(dc, ob);
        _ = DeleteObject(p.into());
    }
}

unsafe fn paint(dc: HDC, fr: &Frame) {
    unsafe {
        // double-buffer: draw into memory, then one blit
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, fr.size.0, fr.size.1);
        let old = SelectObject(mem, bmp.into());
        let key = CreateSolidBrush(COLORREF(KEY));
        FillRect(mem, &RECT { left: 0, top: 0, right: fr.size.0, bottom: fr.size.1 }, key);
        _ = DeleteObject(key.into());
        SetBkMode(mem, TRANSPARENT);
        let null_brush = GetStockObject(NULL_BRUSH);
        match &fr.scene {
            Scene::Arrow { tail, base, tip, ring, ring_col, label } => {
                // arrowhead triangle: tip, and two corners either side of `base`
                let (vx, vy) = ((tip.0 - base.0) as f64, (tip.1 - base.1) as f64);
                let len = (vx * vx + vy * vy).sqrt().max(1.0);
                let (px, py) = (-vy / len, vx / len);
                let head = |w: f64, ext: f64| {
                    [
                        POINT { x: (tip.0 as f64 + vx / len * ext) as i32, y: (tip.1 as f64 + vy / len * ext) as i32 },
                        POINT { x: (base.0 as f64 + px * w) as i32, y: (base.1 as f64 + py * w) as i32 },
                        POINT { x: (base.0 as f64 - px * w) as i32, y: (base.1 as f64 - py * w) as i32 },
                    ]
                };
                for (col, shaft, w, ext) in [(WHITE, 16, 27.0, 6.0), (RED, 10, 22.0, 0.0)] {
                    with(mem, pen(col, shaft), GetStockObject(NULL_BRUSH), || {
                        _ = MoveToEx(mem, tail.0, tail.1, None);
                        _ = LineTo(mem, base.0, base.1);
                    });
                    let br = CreateSolidBrush(COLORREF(col));
                    with(mem, pen(col, 2), br.into(), || _ = Polygon(mem, &head(w, ext)));
                    _ = DeleteObject(br.into());
                }
                with(mem, pen(*ring_col, if *ring_col == GREEN { 6 } else { 4 }), null_brush, || {
                    _ = Ellipse(mem, ring.0 - ring.2, ring.1 - ring.2, ring.0 + ring.2, ring.1 + ring.2);
                });
                if let Some((r, text)) = label {
                    let br = CreateSolidBrush(COLORREF(PANEL));
                    with(mem, pen(RED, 3), br.into(), || {
                        _ = Rectangle(mem, r.left - 12, r.top - 8, r.right + 12, r.bottom + 8);
                    });
                    _ = DeleteObject(br.into());
                    draw_text(mem, text, *r, 0, DT_WORDBREAK);
                }
            }
            Scene::Badge { panel, text, msg, col, dot } => {
                let br = CreateSolidBrush(COLORREF(PANEL));
                with(mem, pen(*col, 3), br.into(), || {
                    _ = Rectangle(mem, panel.left + 1, panel.top + 1, panel.right - 1, panel.bottom - 1);
                });
                _ = DeleteObject(br.into());
                if let Some(d) = dot {
                    let br = CreateSolidBrush(COLORREF(*col));
                    with(mem, pen(*col, 1), br.into(), || _ = Ellipse(mem, d.left, d.top, d.right, d.bottom));
                    _ = DeleteObject(br.into());
                }
                draw_text(mem, msg, *text, 1, DT_SINGLELINE | DT_VCENTER);
            }
        }
        _ = BitBlt(dc, 0, 0, fr.size.0, fr.size.1, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old);
        _ = DeleteObject(bmp.into());
        _ = DeleteDC(mem);
    }
}

unsafe fn draw_text(dc: HDC, text: &str, mut r: RECT, font_idx: usize, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        let f = FONTS.with(|f| if font_idx == 0 { f.borrow().0 } else { f.borrow().1 });
        let old = SelectObject(dc, f.into());
        SetTextColor(dc, COLORREF(WHITE));
        let mut w = win::wide(text);
        w.pop();
        DrawTextW(dc, &mut w, &mut r, flags);
        SelectObject(dc, old);
    }
}
