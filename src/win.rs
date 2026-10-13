//! Small Win32 helpers: cursor, monitors, virtual screen, clipboard, data dir.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub type Rect = (i32, i32, i32, i32);

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe { _ = GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// (monitor rect, work area) of the monitor nearest to (x, y), virtual-screen pixels.
pub fn monitor_at(x: i32, y: i32) -> (Rect, Rect) {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let h = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        _ = GetMonitorInfoW(h, &mut mi);
    }
    let r = |r: RECT| (r.left, r.top, r.right, r.bottom);
    (r(mi.rcMonitor), r(mi.rcWork))
}

/// (x, y, w, h) of the whole virtual desktop.
pub fn virtual_screen() -> Rect {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Replace the clipboard with `text`. Write-only: never reads what was there.
pub fn set_clipboard(text: &str) -> bool {
    let w = wide(text);
    unsafe {
        let mut opened = false;
        for _ in 0..20 {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !opened {
            return false;
        }
        let ok = (|| -> Option<()> {
            EmptyClipboard().ok()?;
            let h = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2).ok()?;
            let p = GlobalLock(h) as *mut u16;
            std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
            _ = GlobalUnlock(h);
            SetClipboardData(13, Some(HANDLE(h.0))).ok()?; // CF_UNICODETEXT; system owns h now
            Some(())
        })()
        .is_some();
        _ = CloseClipboard();
        ok
    }
}

/// `name` in the exe's own folder, which holds token.txt, learned.tsv and marks.tsv: one
/// predictable place (portable-app style) whatever environment launched us.
pub fn data_file(name: &str) -> String {
    let mut buf = [0u16; 1024];
    let n = unsafe { windows::Win32::System::LibraryLoader::GetModuleFileNameW(None, &mut buf) } as usize;
    let exe = String::from_utf16_lossy(&buf[..n]);
    let dir = exe.rsplit_once('\\').map_or(".", |(d, _)| d);
    format!("{dir}\\{name}")
}

pub fn invalidate(hwnd: HWND) {
    unsafe { _ = InvalidateRect(Some(hwnd), None, false) };
}
