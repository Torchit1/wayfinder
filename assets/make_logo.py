"""Draw the Wayfinder logo (a mouse cursor on a red bullseye: "click here") and the app icon.

    py -3 assets/make_logo.py     -> assets/logo.png (512), assets/wayfinder.ico (16/24/32/48)

Drawn at 8x and downscaled for clean anti-aliasing. Small sizes drop the tile so the icon sits
on the taskbar like other tray icons; the cursor keeps a dark outline to read on light or dark.
"""
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
RED = (255, 45, 45, 255)
WHITE = (255, 255, 255, 255)
DARK = (30, 30, 30, 255)

# classic arrow cursor, tip at (0, 0), in 1/12-ish units
CURSOR = [(0, 0), (0, 17), (4.2, 13.2), (7, 19.6), (9.8, 18.4), (7.1, 12.1), (12.4, 12.1)]


def draw(size, tile=True, ss=8):
    S = size * ss
    k = S / 512
    im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    if tile:
        d.rounded_rectangle((0, 0, S - 1, S - 1), radius=104 * k, fill=DARK)
    # bullseye: bigger and thicker without the tile so it survives 16 px
    z = 1.0 if tile else 1.12
    cx, cy = (214 * k, 214 * k) if tile else (196 * k, 196 * k)
    r, w, dot = 132 * k * z, 40 * k * z, 44 * k * z
    if not tile:  # light halo so red reads on a dark taskbar too
        d.ellipse((cx - r - w / 2 - 14 * k, cy - r - w / 2 - 14 * k, cx + r + w / 2 + 14 * k, cy + r + w / 2 + 14 * k),
                  outline=WHITE, width=int(w + 28 * k))
    d.ellipse((cx - r - w / 2, cy - r - w / 2, cx + r + w / 2, cy + r + w / 2), outline=RED, width=int(w))
    d.ellipse((cx - dot, cy - dot, cx + dot, cy + dot), fill=RED)
    # cursor: tip on the bullseye centre, body down-right
    sc = (15.5 if tile else 17.5) * k
    pts = [(cx + x * sc, cy + y * sc) for x, y in CURSOR]
    d.polygon(pts, fill=WHITE, outline=DARK, width=int(9 * k * (1 if tile else 1.6)))
    return im.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    draw(512).save(HERE / "logo.png", optimize=True)
    sizes = [16, 24, 32, 48]
    frames = [draw(s, tile=s >= 32) for s in sizes]
    frames[-1].save(HERE / "wayfinder.ico", format="ICO", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    for f in ("logo.png", "wayfinder.ico"):
        print(f, (HERE / f).stat().st_size, "bytes")
