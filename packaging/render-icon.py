#!/usr/bin/env python3
"""Render the app icon (the design of icons::app_icon in src/icons.rs) for packaging:
nullpdf.png (256 px, the AppImage icon) and nullpdf.ico (16-256 px, the Windows exe icon).

    python3 packaging/render-icon.py     # needs numpy and Pillow

Each size is drawn from the shapes directly, 4x4 supersampled, rather than scaled down
from a big one, so small sizes stay crisp. Keep the shapes in step with app_icon.
"""

from pathlib import Path

import numpy as np
from PIL import Image

SS = 4
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]


def rounded(x, y, mn, mx, r):
    cx, cy = (mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2
    hx, hy = (mx[0] - mn[0]) / 2 - r, (mx[1] - mn[1]) / 2 - r
    dx, dy = np.abs(x - cx) - hx, np.abs(y - cy) - hy
    outside = np.hypot(np.maximum(dx, 0), np.maximum(dy, 0))
    return outside + np.minimum(np.maximum(dx, dy), 0) - r <= 0


def render(size):
    # Sample points in app_icon's 64-unit grid.
    idx = (np.arange(size * SS) + 0.5) / SS * (64.0 / size)
    x, y = np.meshgrid(idx, idx)

    # The red swoosh: a quadratic Bézier, as 24 segments, 2.6 units either side.
    t = np.arange(25) / 24
    u = 1 - t
    (ax, ay), (cx, cy), (bx, by) = (13.0, 47.0), (30.0, 60.0), (51.0, 31.0)
    pts = np.stack([u * u * ax + 2 * u * t * cx + t * t * bx,
                    u * u * ay + 2 * u * t * cy + t * t * by], 1)
    swoosh = np.zeros(x.shape, bool)
    for p, q in zip(pts[:-1], pts[1:]):
        d = q - p
        s = np.clip(((x - p[0]) * d[0] + (y - p[1]) * d[1]) / (d @ d), 0, 1)
        swoosh |= np.hypot(x - (p[0] + d[0] * s), y - (p[1] + d[1] * s)) <= 2.6

    page = rounded(x, y, (18, 10), (46, 54), 3)
    lines = np.zeros(x.shape, bool)
    for k in range(5):
        top = 18 + k * 6
        end = 33 if k == 4 else 40
        lines |= (y >= top) & (y < top + 2.2) & (x >= 23) & (x < end)
    tile = rounded(x, y, (3, 3), (61, 61), 14)

    # Back to front: tile, page, text lines, ink.
    rgba = np.zeros(x.shape + (4,))
    rgba[tile] = (38, 39, 44, 255)
    rgba[page] = (250, 250, 248, 255)
    rgba[page & lines] = (196, 198, 206, 255)
    rgba[swoosh] = (220, 40, 40, 255)

    # Average premultiplied samples, then un-premultiply.
    a = rgba[..., 3:] / 255
    pre = np.concatenate([rgba[..., :3] * a, rgba[..., 3:]], -1)
    pre = pre.reshape(size, SS, size, SS, 4).mean((1, 3))
    alpha = pre[..., 3:]
    rgb = np.where(alpha > 0, pre[..., :3] / np.maximum(alpha / 255, 1e-9), 0)
    px = np.concatenate([rgb, alpha], -1).round().clip(0, 255).astype(np.uint8)
    return Image.fromarray(px, "RGBA")


def main():
    out = Path(__file__).resolve().parent
    render(256).save(out / "nullpdf.png", optimize=True)
    images = [render(s) for s in ICO_SIZES]
    images[-1].save(out / "nullpdf.ico", sizes=[(s, s) for s in ICO_SIZES],
                    append_images=images[:-1])


if __name__ == "__main__":
    main()
