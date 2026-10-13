//! Live index of every interactive control on screen via UI Automation (what screen readers use),
//! so the arrow can be placed by name without a screenshot. Also learns how to reveal hidden
//! controls by noticing which click opened the window they appeared in.

use crate::win::{self, Rect};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
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
    last_click: Mutex<Option<(String, &'static str, Instant)>>,
    ready: AtomicBool,
}

fn learned_path() -> std::path::PathBuf {
    win::data_dir().join("learned.tsv")
}

impl Index {
    pub fn start() -> &'static Index {
        let idx: &'static Index = Box::leak(Box::default());
        if let Ok(s) = std::fs::read_to_string(learned_path()) {
            let mut l = idx.learned.lock().unwrap_or_else(|e| e.into_inner());
            for line in s.lines() {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() == 4 {
                    l.push(Learned {
                        key: f[0].to_lowercase(),
                        name: f[0].into(),
                        opener: f[1].into(),
                        button: f[2].into(),
                        seen: f[3].into(),
                    });
                }
            }
        }
        std::thread::spawn(move || idx.run());
        idx
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        let w = self.wins.lock().unwrap_or_else(|e| e.into_inner());
        (w.len(), w.iter().map(|w| w.items.len()).sum(), self.learned.lock().unwrap_or_else(|e| e.into_inner()).len())
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
            let (born, due) = {
                let mut wins = self.wins.lock().unwrap_or_else(|e| e.into_inner());
                match wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
                    None => (now, true),
                    Some(w) => {
                        let fresh = now - w.born < LEARN_WINDOW; // popups fill in after they appear
                        let every = if hwnd == fg { FG_EVERY } else { OTHER_EVERY };
                        (w.born, w.sig != sig || fresh || now - w.scanned > every)
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
            let mut wins = self.wins.lock().unwrap_or_else(|e| e.into_inner());
            let w = Win { hwnd: hwnd.0 as isize, sig, scanned: Instant::now(), born, items };
            match wins.iter_mut().find(|w| w.hwnd == hwnd.0 as isize) {
                Some(old) => *old = w,
                None => wins.push(w),
            }
        }
        // keep the list in front-to-back order so lookups never need to sort
        let mut wins = self.wins.lock().unwrap_or_else(|e| e.into_inner());
        let mut old = std::mem::take(&mut *wins);
        for h in &live {
            if let Some(i) = old.iter().position(|w| w.hwnd == *h) {
                wins.push(old.swap_remove(i));
            }
        }
    }

    /// A window appeared right after a click: its items are reached via that click.
    fn learn(&self, items: &[Item], born: Instant) {
        let Some((opener, button, t)) = self.last_click.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        if born < t || born - t > LEARN_WINDOW {
            return;
        }
        let mut l = self.learned.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for it in items {
            let key = it.name.to_lowercase();
            if key == opener.to_lowercase() {
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
            let body: String = l
                .iter()
                .map(|r| format!("{}\t{}\t{}\t{}\n", clean(&r.name), clean(&r.opener), r.button, r.seen))
                .collect();
            let tmp = learned_path().with_extension("tmp");
            if std::fs::write(&tmp, body).is_ok() {
                let _ = std::fs::rename(tmp, learned_path());
            }
        }
    }

    /// Called by the overlay's click watcher for every left/right press.
    pub fn on_click(&self, x: i32, y: i32, button: &'static str) {
        if let Some(name) = self.control_at(x, y) {
            *self.last_click.lock().unwrap_or_else(|e| e.into_inner()) = Some((name, button, Instant::now()));
        }
    }

    /// Name of the smallest indexed control under (x, y), frontmost window first.
    pub fn control_at(&self, x: i32, y: i32) -> Option<String> {
        let wins = self.wins.lock().unwrap_or_else(|e| e.into_inner());
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
        let q = query.to_lowercase();
        let wins = self.wins.lock().unwrap_or_else(|e| e.into_inner());
        let mut found = Vec::new();
        for w in wins.iter() {
            for i in &w.items {
                let n = i.name.to_lowercase();
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
            let l = self.learned.lock().unwrap_or_else(|e| e.into_inner());
            let q = query.to_lowercase();
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

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn today() -> String {
    let st = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02}", st.wYear, st.wMonth, st.wDay)
}
