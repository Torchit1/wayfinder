//! Whole-desktop screenshot, downscaled, mouse marked, PNG-encoded with Windows' own WIC.

use crate::win;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::*;

pub struct Shot {
    pub png: Vec<u8>,
    pub scale: f64,
    pub origin: (i32, i32),
    pub size: (u32, u32),
    pub cursor: (i32, i32),
    pub monitor: win::Rect,
}

/// Capture, downscale to `width` px wide (never upscales), mark the cursor. None if capture is
/// blocked (UAC secure desktop).
pub fn take(width: u32) -> Option<Shot> {
    let (vx, vy, vw, vh) = win::virtual_screen();
    let (cx, cy) = win::cursor();
    let (mon, _) = win::monitor_at(cx, cy);
    let src = capture(vx, vy, vw, vh)?;
    let dw = width.min(vw as u32).max(1);
    let scale = vw as f64 / dw as f64;
    let dh = ((vh as f64 / scale).round() as u32).max(1);
    let mut rgb = downscale(&src, vw as usize, vh as usize, dw as usize, dh as usize);
    let (sx, sy) = ((cx - vx) as f64 / scale, (cy - vy) as f64 / scale);
    mark(&mut rgb, dw as usize, dh as usize, sx, sy);
    let png = encode_png(&rgb, dw, dh)?;
    let m = |v: i32, o: i32| ((v - o) as f64 / scale).round() as i32;
    Some(Shot {
        png,
        scale,
        origin: (vx, vy),
        size: (dw, dh),
        cursor: (sx.round() as i32, sy.round() as i32),
        monitor: (m(mon.0, vx), m(mon.1, vy), m(mon.2, vx), m(mon.3, vy)),
    })
}

/// BGRA top-down pixels of the given virtual-screen rect, including layered windows (the arrow).
fn capture(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT).is_ok();
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS);
        SelectObject(mem, old);
        _ = DeleteObject(bmp.into());
        _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        (ok && lines == h).then_some(buf)
    }
}

/// Box-filter BGRA -> BGR (what WIC's 24bppBGR wants).
fn downscale(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 3];
    for dy in 0..dh {
        let (y0, y1) = (dy * sh / dh, ((dy + 1) * sh / dh).max(dy * sh / dh + 1));
        for dx in 0..dw {
            let (x0, x1) = (dx * sw / dw, ((dx + 1) * sw / dw).max(dx * sw / dw + 1));
            let mut acc = [0u32; 3];
            for y in y0..y1 {
                let row = &src[(y * sw + x0) * 4..(y * sw + x1) * 4];
                for p in row.chunks_exact(4) {
                    acc[0] += p[0] as u32;
                    acc[1] += p[1] as u32;
                    acc[2] += p[2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (dy * dw + dx) * 3;
            out[o..o + 3].copy_from_slice(&[(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8]);
        }
    }
    out
}

/// Magenta ring + crosshair where the mouse is (screen grabs don't include the cursor).
fn mark(rgb: &mut [u8], w: usize, h: usize, cx: f64, cy: f64) {
    let mut put = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            let o = (y as usize * w + x as usize) * 3;
            rgb[o..o + 3].copy_from_slice(&[255, 0, 255]);
        }
    };
    let (ix, iy) = (cx.round() as i64, cy.round() as i64);
    for dy in -18..=18i64 {
        for dx in -18..=18i64 {
            let d = ((dx * dx + dy * dy) as f64).sqrt();
            if (10.5..=13.5).contains(&d) || (dy.abs() <= 1 && dx.abs() <= 18) || (dx.abs() <= 1 && dy.abs() <= 18) {
                put(ix + dx, iy + dy);
            }
        }
    }
}

fn encode_png(bgr: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = CreateStreamOnHGlobal(Default::default(), true).ok()?;
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut()).ok()?;
        let frame = frame?;
        frame.Initialize(None).ok()?;
        frame.SetSize(w, h).ok()?;
        let mut fmt = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut fmt).ok()?;
        frame.WritePixels(h, w * 3, bgr).ok()?;
        frame.Commit().ok()?;
        enc.Commit().ok()?;
        let hg = GetHGlobalFromStream(&stream).ok()?;
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).ok()?;
        let n = stat.cbSize as usize;
        let p = GlobalLock(hg) as *const u8;
        let out = std::slice::from_raw_parts(p, n).to_vec();
        _ = GlobalUnlock(hg);
        Some(out)
    }
}
