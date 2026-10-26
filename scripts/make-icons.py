#!/usr/bin/env python3
"""Make every kimo icon from the master logo (assets/brand/kimo-1024.png, ink on transparent).

    python3 scripts/make-icons.py      # needs Pillow

Writes the web client icons (web/icons), raw RGBA icons for the native window
(assets/icons/*.rgba, loaded without an image decoder) and, on macOS, the app icon
(assets/icons/kimo.icns, with iconutil)."""
import shutil
import subprocess
import tempfile
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
LOGO = Image.open(ROOT / "assets/brand/kimo-1024.png").convert("RGBA")
WHITE = (255, 255, 255, 255)


def on_square(size, fill, scale):
    """The logo centred on a square; `scale` = share of the side the logo may use."""
    canvas = Image.new("RGBA", (size, size), fill)
    w, h = LOGO.size
    k = size * scale / max(w, h)
    logo = LOGO.resize((round(w * k), round(h * k)), Image.LANCZOS)
    canvas.alpha_composite(logo, ((size - logo.width) // 2, (size - logo.height) // 2))
    return canvas


icons = ROOT / "web/icons"
icons.mkdir(parents=True, exist_ok=True)
# Maskable app icons: the logo stays inside the 80 % safe zone.
on_square(512, WHITE, 0.62).save(icons / "icon-512.png", optimize=True)
on_square(192, WHITE, 0.62).save(icons / "icon-192.png", optimize=True)
on_square(180, WHITE, 0.72).save(icons / "apple-touch-icon.png", optimize=True)
on_square(96, (0, 0, 0, 0), 0.9).save(icons / "badge-96.png", optimize=True)

raw = ROOT / "assets/icons"
raw.mkdir(parents=True, exist_ok=True)
(raw / "window-128.rgba").write_bytes(on_square(128, WHITE, 0.78).tobytes())
(raw / "logo-64.rgba").write_bytes(on_square(64, (0, 0, 0, 0), 0.96).tobytes())

# macOS app icon: the logo on a white rounded square (824 of 1024, Apple's grid), with a
# soft edge so it reads on dark Docks too. `dark`: a white logo on a near-black square.
def mac_icon(size, dark=False):
    scale = size / 1024
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    inset, side = round(100 * scale), round(824 * scale)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (inset, inset, inset + side - 1, inset + side - 1), radius=round(185 * scale), fill=255)
    tile = Image.new("RGBA", (size, size), (28, 28, 28, 255) if dark else WHITE)
    canvas.paste(tile, (0, 0), mask)
    ImageDraw.Draw(canvas).rounded_rectangle(
        (inset, inset, inset + side - 1, inset + side - 1), radius=round(185 * scale),
        outline=(255, 255, 255, 50) if dark else (0, 0, 0, 40), width=max(1, round(2 * scale)))
    w, h = LOGO.size
    k = side * 0.62 / max(w, h)
    logo = LOGO.resize((max(1, round(w * k)), max(1, round(h * k))), Image.LANCZOS)
    if dark:
        white = Image.new("RGBA", logo.size, WHITE)
        white.putalpha(logo.getchannel("A"))
        logo = white
    canvas.alpha_composite(logo, ((size - logo.width) // 2, (size - logo.height) // 2))
    return canvas


# The window's icon on macOS: the app icon's rounded square (the Dock shows it while kimo runs).
(raw / "mac-256.rgba").write_bytes(mac_icon(256).tobytes())
(raw / "mac-256-dark.rgba").write_bytes(mac_icon(256, dark=True).tobytes())

if shutil.which("iconutil"):
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "kimo.iconset"
        iconset.mkdir()
        for dark, name in ((False, "kimo.icns"), (True, "kimo-dark.icns")):
            for px in (16, 32, 128, 256, 512):
                mac_icon(px, dark).save(iconset / f"icon_{px}x{px}.png")
                mac_icon(px * 2, dark).save(iconset / f"icon_{px}x{px}@2x.png")
            subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(raw / name)],
                           check=True)
else:
    print("no iconutil (macOS only): assets/icons/kimo.icns left as it is")
print("icons written")
