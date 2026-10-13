//! Live index of every interactive control on screen via UI Automation (what screen readers use),
//! so the arrow can be placed by name without a screenshot. Also learns how to reveal hidden
//! controls by noticing which click opened the window they appeared in, checks those routes and
//! forgets ones that stop working, and remembers named spots the index can't see (anchored to a
//! nearby named control, so they follow layout changes).

use crate::win::{self, Rect};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

const TICK: Duration = Duration::from_millis(250);
const FG_EVERY: Duration = Duration::from_secs(1);
const OTHER_EVERY: Duration = Duration::from_secs(10);
const LEARN_WINDOW: Duration = Duration::from_secs(2);
/// After the person clicks a route's first step, its target must show up within this long.
const VERIFY: Duration = Duration::from_secs(4);

const INTERACTIVE: [UIA_CONTROLTYPE_ID; 12] = [
    UIA_ButtonControlTypeId,
    UIA_MenuItemControlTypeId,
    UIA_ListItemControlTypeId,
    UIA_TreeItemControlTypeId,
    UIA_TabItemControlTypeId,
    UIA_HyperlinkControlTypeId,
    UIA_EditControlTypeId,
    UIA_ComboBoxControlTypeId,
    UIA_CheckBoxControlTypeId,
    UIA_RadioButtonControlTypeId,
    UIA_SplitButtonControlTypeId,
    UIA_DataItemControlTypeId,
];

struct Item {
    name: String,
    ctype: String,
    rect: Rect,
}

struct Win {
    hwnd: isize,
    app: String,
    sig: (Rect, String),
    scanned: Instant,
    born: Instant,
    items: Vec<Item>,
}

#[derive(Clone)]
pub struct Learned {
    key: String,
    pub name: String,
    pub opener: String,
    pub button: String,
    pub seen: String,
}

/// A spot the index can't see, remembered by name: offset from the nearest uniquely-named
/// control in the same app window, or (if the window had none) a fraction of the window.
#[derive(Clone)]
struct Mark {
    key: String,
    name: String,
    app: String,
    anchor: String,
    dx: i32,
    dy: i32,
    fx: i32,
    fy: i32,
}

#[derive(Clone)]
pub struct Match {
    pub name: String,
    pub ctype: String,
    pub rect: Rect,
    pub center: (i32, i32),
    pub window: String,
    pub score: i32,
}

#[derive(Default)]
pub struct Index {
    wins: Mutex<Vec<Win>>,
    learned: Mutex<Vec<Learned>>,
    marks: Mutex<Vec<Mark>>,
    last_click: Mutex<Option<(String, &'static str, Instant)>>,
    /// route being followed: (first step, target key, when the first step was clicked)
    expect: Mutex<Option<(String, String, Option<Instant>)>>,
    ready: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn rows(file: &str) -> Vec<Vec<String>> {
    let s = std::fs::read_to_string(win::data_file(file)).unwrap_or_default();
    // tolerate files re-saved by Windows tools: UTF-8 BOM, CRLF
    s.trim_start_matches('\u{feff}').lines().map(|l| l.trim_end_matches('\r').split('\t').map(String::from).collect()).collect()
}

fn save(file: &str, body: String) {
    let path = win::data_file(file);
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

fn center(r: Rect) -> (i32, i32) {
    ((r.0 + r.2) / 2, (r.1 + r.3) / 2)
}

fn int(s: &str) -> i32 {
    crate::json::int(s).unwrap_or(0) as i32
}

impl Index {
    pub fn start() -> &'static Index {
        let idx: &'static Index = Box::leak(Box::default());
        for f in rows("learned.tsv").into_iter().filter(|f| f.len() == 4) {
            lock(&idx.learned).push(Learned {
                key: f[0].to_ascii_lowercase(),
                name: f[0].clone(),
                opener: f[1].clone(),
                button: f[2].clone(),
                seen: f[3].clone(),
            });
        }
        for f in rows("marks.tsv").into_iter().filter(|f| f.len() == 7) {
            lock(&idx.marks).push(Mark {
                key: f[0].to_ascii_lowercase(),
                name: f[0].clone(),
                app: f[1].clone(),
                anchor: f[2].clone(),
                dx: int(&f[3]),
                dy: int(&f[4]),
                fx: int(&f[5]),
                fy: int(&f[6]),
            });
        }
        std::thread::spawn(move || idx.run());
        idx
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let w = lock(&self.wins);
        (w.len(), w.iter().map(|w| w.items.len()).sum(), lock(&self.learned).len())
    }

    fn run(&self) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(u): windows::core::Result<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            else {
                return;
            };
            let Some((cond, cache)) = build_query(&u) else { return };
            loop {
                self.cycle(&u, &cond, &cache);
                self.ready.store(true, Ordering::Relaxed);
                std::thread::sleep(TICK);
            }
        }
    }

    unsafe fn cycle(&self, u: &IUIAutomation, cond: &IUIAutomationCondition, cache: &IUIAutomationCacheRequest) {
        let now = Instant::now();
        let fg = unsafe { GetForegroundWindow() };
        let mut live = Vec::new();
        for hwnd in top_windows() {
            let mut r = RECT::default();
            unsafe { _ = GetWindowRect(hwnd, &mut r) };
            if r.right <= r.left || r.bottom <= r.top {
                continue;
            }
            live.push(hwnd.0 as isize);
            let sig = ((r.left, r.top, r.right, r.bottom), window_title(hwnd));
            let (born, due, app) = {
                let mut wins = lock(&self.wins);
                match wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
                    None => (now, true, exe_name(hwnd)),
                    Some(w) => {
                        let fresh = now - w.born < LEARN_WINDOW; // popups fill in after they appear
                        let every = if hwnd == fg { FG_EVERY } else { OTHER_EVERY };
                        (w.born, w.sig != sig || fresh || now - w.scanned > every, w.app.clone())
                    }
                }
            };
            if !due {
                continue;
            }
            let items = unsafe { scan(u, hwnd, cond, cache) };
            let learn = now - born < LEARN_WINDOW && !items.is_empty() && self.ready.load(Ordering::Relaxed);
            if learn {
                self.learn(&items, born);
            }
            let mut wins = lock(&self.wins);
            let w = Win { hwnd: hwnd.0 as isize, app, sig, scanned: Instant::now(), born, items };
            match wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
                Some(old) => *old = w,
                None => wins.push(w),
            }
        }
        // keep the list in front-to-back order so lookups never need to sort
        {
            let mut wins = lock(&self.wins);
            let mut old = std::mem::take(&mut *wins);
            for h in &live {
                if let Some(i) = old.iter().position(|w| w.hwnd == *h) {
                    wins.push(old.swap_remove(i));
                }
            }
        }
        // a followed route: its target appeared (good) or didn't in time (the app changed: forget it)
        let check = lock(&self.expect).clone();
        if let Some((_, target, Some(t))) = check {
            if !self.lookup(&target, 1).is_empty() {
                *lock(&self.expect) = None;
            } else if t.elapsed() > VERIFY {
                let mut l = lock(&self.learned);
                l.retain(|r| r.key != target);
                save_learned(&l);
                *lock(&self.expect) = None;
            }
        }
    }

    /// The agent is sending the person through a learned route; check it on their click.
    pub fn expect(&self, first_step: &str, target: &str) {
        *lock(&self.expect) = Some((first_step.into(), target.to_ascii_lowercase(), None));
    }

    /// Remember a spot the index can't see (e.g. one the agent found in a screenshot) by name.
    pub fn remember(&self, name: &str, x: i32, y: i32) -> bool {
        let inside = |r: Rect| r.0 <= x && x < r.2 && r.1 <= y && y < r.3;
        let mark = {
            let wins = lock(&self.wins);
            let Some(w) = wins.iter().find(|w| inside(w.sig.0)) else { return false };
            let r = w.sig.0;
            // anchor: nearest control whose name is unique in the window, so it's findable again
            let anchor = w
                .items
                .iter()
                .filter(|i| w.items.iter().filter(|j| j.name == i.name).count() == 1)
                .min_by_key(|i| {
                    let c = center(i.rect);
                    ((c.0 - x) as i64).pow(2) + ((c.1 - y) as i64).pow(2)
                });
            let (anchor, dx, dy) = anchor.map_or((String::new(), 0, 0), |a| {
                let c = center(a.rect);
                (a.name.clone(), x - c.0, y - c.1)
            });
            Mark {
                key: name.to_ascii_lowercase(),
                name: name.into(),
                app: w.app.clone(),
                anchor,
                dx,
                dy,
                fx: (x - r.0) * 10000 / (r.2 - r.0).max(1),
                fy: (y - r.1) * 10000 / (r.3 - r.1).max(1),
            }
        };
        let mut m = lock(&self.marks);
        m.retain(|k| !(k.key == mark.key && k.app == mark.app));
        m.push(mark);
        save(
            "marks.tsv",
            m.iter()
                .map(|k| format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\n", clean(&k.name), k.app, clean(&k.anchor), k.dx, k.dy, k.fx, k.fy))
                .collect(),
        );
        true
    }

    /// Where a remembered spot is now: follows its anchor (preferring the mouse's monitor); never
    /// guesses if the anchor is gone. Anchor names like "X - 2 running windows" drift, so if the
    /// exact name is missing, the stable part before " - " is tried.
    pub fn recall(&self, query: &str) -> Option<(String, (i32, i32))> {
        let q = query.to_ascii_lowercase();
        let mark = {
            let m = lock(&self.marks);
            m.iter().find(|k| k.key == q).or_else(|| m.iter().find(|k| k.key.contains(&q))).cloned()
        }?;
        let (cx, cy) = win::cursor();
        let (mon, _) = win::monitor_at(cx, cy);
        let stable = mark.anchor.split(" - ").next().unwrap_or("");
        let wins = lock(&self.wins);
        let mut spots = Vec::new();
        for w in wins.iter().filter(|w| w.app == mark.app) {
            let r = w.sig.0;
            let p = if mark.anchor.is_empty() {
                Some((r.0 + (r.2 - r.0) * mark.fx / 10000, r.1 + (r.3 - r.1) * mark.fy / 10000))
            } else {
                let a = w.items.iter().find(|i| i.name == mark.anchor).or_else(|| w.items.iter().find(|i| !stable.is_empty() && i.name.starts_with(stable)));
                a.map(|a| (center(a.rect).0 + mark.dx, center(a.rect).1 + mark.dy))
            };
            if let Some(p) = p.filter(|p| r.0 <= p.0 && p.0 < r.2 && r.1 <= p.1 && p.1 < r.3) {
                spots.push(p);
            }
        }
        let on_mon = |p: &&(i32, i32)| mon.0 <= p.0 && p.0 < mon.2 && mon.1 <= p.1 && p.1 < mon.3;
        let p = *spots.iter().find(on_mon).or(spots.first())?;
        Some((mark.name, p))
    }

    /// A window appeared right after a click: its items are reached via that click.
    fn learn(&self, items: &[Item], born: Instant) {
        let Some((opener, button, t)) = lock(&self.last_click).clone() else { return };
        if born < t || born - t > LEARN_WINDOW {
            return;
        }
        let mut l = lock(&self.learned);
        let mut changed = false;
        for it in items {
            let key = it.name.to_ascii_lowercase();
            if key == opener.to_ascii_lowercase() {
                continue;
            }
            let rec = Learned { key: key.clone(), name: it.name.clone(), opener: opener.clone(), button: button.into(), seen: today() };
            match l.iter_mut().find(|r| r.key == key) {
                Some(r) if r.opener == opener && r.button == button => {}
                Some(r) => {
                    *r = rec;
                    changed = true;
                }
                None => {
                    l.push(rec);
                    changed = true;
                }
            }
        }
        if changed {
            save_learned(&l);
        }
    }

    /// Called by the overlay's click watcher for every left/right press.
    pub fn on_click(&self, x: i32, y: i32, button: &'static str) {
        if let Some(name) = self.control_at(x, y) {
            if let Some(e) = lock(&self.expect).as_mut() {
                if e.0 == name {
                    e.2 = Some(Instant::now());
                }
            }
            *lock(&self.last_click) = Some((name, button, Instant::now()));
        }
    }

    /// Name of the smallest indexed control under (x, y), frontmost window first.
    pub fn control_at(&self, x: i32, y: i32) -> Option<String> {
        let wins = lock(&self.wins);
        wins.iter().find_map(|w| {
            w.items
                .iter()
                .filter(|i| i.rect.0 <= x && x < i.rect.2 && i.rect.1 <= y && y < i.rect.3)
                .min_by_key(|i| (i.rect.2 - i.rect.0) as i64 * (i.rect.3 - i.rect.1) as i64)
                .map(|i| i.name.clone())
        })
    }

    /// Visible controls whose name contains `query`, best first: exact > prefix > substring,
    /// then on the mouse's monitor, then front-to-back.
    pub fn lookup(&self, query: &str, limit: usize) -> Vec<Match> {
        let (cx, cy) = win::cursor();
        let (mon, _) = win::monitor_at(cx, cy);
        let q = query.to_ascii_lowercase();
        let wins = lock(&self.wins);
        let mut found = Vec::new();
        for w in wins.iter() {
            for i in &w.items {
                let n = i.name.to_ascii_lowercase();
                if !n.contains(&q) {
                    continue;
                }
                let center = ((i.rect.0 + i.rect.2) / 2, (i.rect.1 + i.rect.3) / 2);
                let kind = if n == q { 3 } else if n.starts_with(&q) { 2 } else { 1 };
                let on_mon = mon.0 <= center.0 && center.0 < mon.2 && mon.1 <= center.1 && center.1 < mon.3;
                found.push(Match {
                    name: i.name.clone(),
                    ctype: i.ctype.clone(),
                    rect: i.rect,
                    center,
                    window: w.sig.1.clone(),
                    score: kind * 2 + on_mon as i32,
                });
            }
        }
        // best score first, front-to-back within a score (scores are 2..=7: bucket, don't sort)
        let mut found: Vec<Match> = (2..=7).rev().flat_map(|sc| found.iter().filter(move |m| m.score == sc)).cloned().collect();
        found.truncate(limit);
        found
    }

    /// Learned steps to reveal a hidden control: [(opener, button), ..., (target, "")].
    pub fn route(&self, query: &str, depth: u32) -> Option<Vec<(String, String)>> {
        if depth == 0 {
            return None;
        }
        let rec = {
            let l = lock(&self.learned);
            let q = query.to_ascii_lowercase();
            l.iter().find(|r| r.key == q).or_else(|| l.iter().find(|r| r.key.contains(&q))).cloned()
        }?;
        let tail = vec![(rec.opener.clone(), rec.button.clone()), (rec.name.clone(), String::new())];
        if !self.lookup(&rec.opener, 1).is_empty() {
            return Some(tail);
        }
        let mut before = self.route(&rec.opener, depth - 1)?;
        before.pop();
        before.extend(tail);
        Some(before)
    }
}

unsafe fn build_query(u: &IUIAutomation) -> Option<(IUIAutomationCondition, IUIAutomationCacheRequest)> {
    unsafe {
        let types: Vec<Option<IUIAutomationCondition>> = INTERACTIVE
            .iter()
            .map(|t| u.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(t.0)).ok())
            .collect();
        let any = u.CreateOrConditionFromNativeArray(&types).ok()?;
        let onscreen = u.CreatePropertyCondition(UIA_IsOffscreenPropertyId, &VARIANT::from(false)).ok()?;
        let cond = u.CreateAndCondition(&onscreen, &any).ok()?;
        let cache = u.CreateCacheRequest().ok()?;
        for p in [UIA_NamePropertyId, UIA_LocalizedControlTypePropertyId, UIA_BoundingRectanglePropertyId] {
            cache.AddProperty(p).ok()?;
        }
        Some((cond, cache))
    }
}

unsafe fn scan(u: &IUIAutomation, hwnd: HWND, cond: &IUIAutomationCondition, cache: &IUIAutomationCacheRequest) -> Vec<Item> {
    let mut items = Vec::new();
    unsafe {
        let Ok(el) = u.ElementFromHandle(hwnd) else { return items };
        let Ok(arr) = el.FindAllBuildCache(TreeScope_Subtree, cond, cache) else { return items };
        for i in 0..arr.Length().unwrap_or(0) {
            let Ok(e) = arr.GetElement(i) else { continue };
            let name = e.CachedName().map(|b| b.to_string()).unwrap_or_default();
            let Ok(b) = e.CachedBoundingRectangle() else { continue };
            if !name.is_empty() && b.right > b.left && b.bottom > b.top {
                let ctype = e.CachedLocalizedControlType().map(|b| b.to_string()).unwrap_or_default();
                items.push(Item { name, ctype, rect: (b.left, b.top, b.right, b.bottom) });
            }
        }
    }
    items
}

/// Actually-visible top-level windows of other processes, front to back (EnumWindows is z-order).
fn top_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(lp.0 as *mut Vec<HWND>);
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() && !cloaked(h) && pid != GetCurrentProcessId() {
                out.push(h);
            }
        }
        true.into()
    }
    let mut out: Vec<HWND> = Vec::new();
    unsafe { _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize)) };
    out
}

/// Windows DWM keeps hidden but "visible": the closed Start menu, other virtual desktops.
fn cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    unsafe { _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4) };
    v != 0
}

fn window_title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(h, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn save_learned(l: &[Learned]) {
    save("learned.tsv", l.iter().map(|r| format!("{}\t{}\t{}\t{}\n", clean(&r.name), clean(&r.opener), r.button, r.seen)).collect());
}

/// Lower-case exe file name of the window's process ("chrome.exe"); "" if it can't be read
/// (e.g. an admin process while we run unelevated).
fn exe_name(h: HWND) -> String {
    use windows::Win32::System::Threading::*;
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 260];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        _ = CloseHandle(p);
        if !ok {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..n as usize]).rsplit('\\').next().unwrap_or("").to_ascii_lowercase()
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn today() -> String {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}
