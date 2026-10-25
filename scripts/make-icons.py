#!/usr/bin/env python3
"""Make every kimo icon from the master logo (assets/brand/kimo-1024.png, ink on transparent).

    python3 scripts/make-icons.py      # needs Pillow

Writes the web client icons (web/icons) and raw RGBA icons for the native window
(assets/icons/*.rgba, loaded without an image decoder)."""
from pathlib import Path
from PIL import Image

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
print("icons written")
