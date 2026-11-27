#!/usr/bin/env python3
"""Generate crisp SDF-rendered PNG assets for Microsoft Store & MSIX packaging.

Uses only Python standard library modules (`math`, `os`, `struct`, `zlib`)
so asset generation runs cleanly everywhere without Pillow/system dependencies.
The icon geometry matches `build_app_icon()` in `src/main.rs`.
"""

from __future__ import annotations

import math
import os
import struct
import zlib

ROOT = os.path.dirname(os.path.abspath(__file__))
ASSETS_DIR = os.path.join(ROOT, "Assets")
STORE_DIR = os.path.join(os.path.dirname(ROOT), "store")


def sample_icon_rgba(nx: float, ny: float) -> tuple[float, float, float, float]:
    """Evaluate normalized SDF icon at `(nx, ny)` in `[0.0, 1.0]^2`."""
    # Map normalized [0, 1] square onto 32x32 design coordinates
    x = nx * 32.0
    y = ny * 32.0
    center = 16.0
    dx = x - center
    dy = y - center
    dist = math.hypot(dx, dy)

    # Rounded square SDF matching src/main.rs (`10.5 + 4.0 = 14.5` half-extent)
    qx = abs(dx) - 10.5
    qy = abs(dy) - 10.5
    box_dist = math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - 4.0

    # Smooth AA edge width (calibrated for resolution-independent crispness)
    edge = 0.28
    if box_dist > edge:
        return (0.0, 0.0, 0.0, 0.0)

    aa = max(0.0, min(1.0, (edge - box_dist) / (2.0 * edge)))
    t = max(0.0, min(1.0, (x + y) / 64.0))
    r = 249.0 - t * 25.0
    g = 115.0 - t * 40.0
    b = 22.0 - t * 10.0

    # Crisp white recovery ring with open arc notch on top-right
    ring_dist = abs(dist - 6.8)
    in_notch = dx > 1.0 and dy < -1.0
    if not in_notch and ring_dist < 1.35:
        ring_aa = max(0.0, min(1.0, (1.35 - ring_dist) / 0.25))
        r = r * (1.0 - ring_aa) + 255.0 * ring_aa
        g = g * (1.0 - ring_aa) + 255.0 * ring_aa
        b = b * (1.0 - ring_aa) + 255.0 * ring_aa
    elif dist < 2.3 and dist <= 14.5:
        dot_aa = max(0.0, min(1.0, (2.3 - dist) / 0.25))
        r = r * (1.0 - dot_aa) + 255.0 * dot_aa
        g = g * (1.0 - dot_aa) + 237.0 * dot_aa
        b = b * (1.0 - dot_aa) + 213.0 * dot_aa

    return (r, g, b, aa * 255.0)


def render_rgba(width: int, height: int, padding_ratio: float = 0.0, supersample: int = 4) -> bytes:
    """Render an `RGBA` buffer of `width x height`, centering icon in safe box."""
    side = min(width, height) * (1.0 - 2.0 * padding_ratio)
    ox = (width - side) * 0.5
    oy = (height - side) * 0.5
    inv_ss = 1.0 / supersample
    inv_ss2 = 1.0 / (supersample * supersample)

    rows = bytearray()
    for py in range(height):
        rows.append(0)  # PNG filter type 0 (None)
        for px in range(width):
            sr = sg = sb = sa = 0.0
            for sy in range(supersample):
                y_pos = py + (sy + 0.5) * inv_ss
                ny = (y_pos - oy) / side
                for sx in range(supersample):
                    x_pos = px + (sx + 0.5) * inv_ss
                    nx = (x_pos - ox) / side
                    if 0.0 <= nx <= 1.0 and 0.0 <= ny <= 1.0:
                        r, g, b, a = sample_icon_rgba(nx, ny)
                        # Premultiply alpha while averaging subpixels
                        af = a / 255.0
                        sr += r * af
                        sg += g * af
                        sb += b * af
                        sa += a
            avg_a = sa * inv_ss2
            if avg_a > 0.5:
                af = avg_a / 255.0
                out_r = min(255, int(round((sr * inv_ss2) / af)))
                out_g = min(255, int(round((sg * inv_ss2) / af)))
                out_b = min(255, int(round((sb * inv_ss2) / af)))
                out_a = min(255, int(round(avg_a)))
            else:
                out_r = out_g = out_b = out_a = 0
            rows.extend((out_r, out_g, out_b, out_a))
    return bytes(rows)


def write_png(path: str, width: int, height: int, raw_scanlines: bytes) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    compressed = zlib.compress(raw_scanlines, level=9)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", compressed) + chunk(b"IEND", b"")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(png)


def emit(path: str, width: int, height: int, padding_ratio: float = 0.0, ss: int = 4) -> None:
    raw = render_rgba(width, height, padding_ratio=padding_ratio, supersample=ss)
    write_png(path, width, height, raw)
    print(f"wrote {os.path.relpath(path, ROOT)} ({width}x{height})")


def main() -> None:
    os.makedirs(ASSETS_DIR, exist_ok=True)
    os.makedirs(STORE_DIR, exist_ok=True)

    # Partner Center Store Listing 1:1 Logo (300x300)
    emit(os.path.join(STORE_DIR, "StoreLogo_300x300.png"), 300, 300, padding_ratio=0.04, ss=4)

    # Package StoreLogo (base 50x50 + standard scales)
    store_scales = {100: 50, 125: 63, 150: 75, 200: 100, 400: 200}
    emit(os.path.join(ASSETS_DIR, "StoreLogo.png"), 50, 50, padding_ratio=0.04, ss=4)
    for scale, px in store_scales.items():
        emit(os.path.join(ASSETS_DIR, f"StoreLogo.scale-{scale}.png"), px, px, padding_ratio=0.04, ss=4)

    # Square44x44Logo (Start App List / Taskbar)
    sq44_scales = {100: 44, 125: 55, 150: 66, 200: 88, 400: 176}
    emit(os.path.join(ASSETS_DIR, "Square44x44Logo.png"), 44, 44, padding_ratio=0.04, ss=4)
    for scale, px in sq44_scales.items():
        emit(os.path.join(ASSETS_DIR, f"Square44x44Logo.scale-{scale}.png"), px, px, padding_ratio=0.04, ss=4)

    for target in (16, 24, 32, 48, 256):
        ss = 4 if target <= 64 else 3
        emit(os.path.join(ASSETS_DIR, f"Square44x44Logo.targetsize-{target}.png"), target, target, padding_ratio=0.03, ss=ss)
        emit(
            os.path.join(ASSETS_DIR, f"Square44x44Logo.targetsize-{target}_altform-unplated.png"),
            target,
            target,
            padding_ratio=0.03,
            ss=ss,
        )

    # Square150x150Logo (Medium Tile)
    sq150_scales = {100: 150, 125: 188, 150: 225, 200: 300, 400: 600}
    emit(os.path.join(ASSETS_DIR, "Square150x150Logo.png"), 150, 150, padding_ratio=0.14, ss=3)
    for scale, px in sq150_scales.items():
        ss = 2 if px >= 500 else 3
        emit(os.path.join(ASSETS_DIR, f"Square150x150Logo.scale-{scale}.png"), px, px, padding_ratio=0.14, ss=ss)

    # Square71x71Logo (Small Tile)
    sq71_scales = {100: 71, 125: 89, 150: 107, 200: 142, 400: 284}
    emit(os.path.join(ASSETS_DIR, "Square71x71Logo.png"), 71, 71, padding_ratio=0.12, ss=3)
    for scale, px in sq71_scales.items():
        emit(os.path.join(ASSETS_DIR, f"Square71x71Logo.scale-{scale}.png"), px, px, padding_ratio=0.12, ss=3)

    # Wide310x150Logo (Wide Tile)
    wide_scales = {100: (310, 150), 125: (388, 188), 150: (465, 225), 200: (620, 300), 400: (1240, 600)}
    emit(os.path.join(ASSETS_DIR, "Wide310x150Logo.png"), 310, 150, padding_ratio=0.14, ss=3)
    for scale, (w, h) in wide_scales.items():
        ss = 2 if w >= 600 else 3
        emit(os.path.join(ASSETS_DIR, f"Wide310x150Logo.scale-{scale}.png"), w, h, padding_ratio=0.14, ss=ss)

    # Square310x310Logo (Large Tile)
    sq310_scales = {100: 310, 125: 388, 150: 465, 200: 620, 400: 1240}
    emit(os.path.join(ASSETS_DIR, "Square310x310Logo.png"), 310, 310, padding_ratio=0.16, ss=2)
    for scale, px in sq310_scales.items():
        ss = 2
        emit(os.path.join(ASSETS_DIR, f"Square310x310Logo.scale-{scale}.png"), px, px, padding_ratio=0.16, ss=ss)


if __name__ == "__main__":
    main()
