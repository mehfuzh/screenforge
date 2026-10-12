#!/usr/bin/env python3
"""Generate ScreenForge's own wallpaper set: soft mesh gradients (blended colour fields with
a gentle wave warp and film grain). Original artwork, free to ship with the app.

    python3 scripts/gen-wallpapers.py        # writes assets/backgrounds/<Category>/<name>.jpg
"""
import math, os, random
import numpy as np
from PIL import Image

W, H = 2880, 1800
OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "backgrounds")

def hex2rgb(h):
    h = h.lstrip("#"); return np.array([int(h[i:i+2], 16) for i in (0, 2, 4)], dtype=np.float32) / 255.0

def mesh(base, blobs, seed, warp=0.08):
    rnd = random.Random(seed)
    # low-res field, upscaled at the end (smooth by construction)
    w, h = W // 4, H // 4
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    u, v = x / w, y / h
    # wave warp for organic shapes
    p1, p2 = rnd.random() * 6.28, rnd.random() * 6.28
    uu = u + warp * np.sin(v * 3.1 + p1) + warp * 0.5 * np.sin(v * 7.3 + p2)
    vv = v + warp * np.sin(u * 2.7 + p2) + warp * 0.5 * np.sin(u * 5.9 + p1)
    img = np.ones((h, w, 3), np.float32) * hex2rgb(base)
    for color, (cx, cy, r) in blobs:
        d2 = ((uu - cx) * 1.6) ** 2 + (vv - cy) ** 2
        a = np.exp(-d2 / (2 * r * r))[..., None]
        img = img * (1 - a) + hex2rgb(color) * a
    im = Image.fromarray((np.clip(img, 0, 1) * 255).astype(np.uint8)).resize((W, H), Image.BICUBIC)
    arr = np.asarray(im).astype(np.float32)
    grain = np.random.default_rng(seed).normal(0, 2.2, (H, W, 1)).astype(np.float32)
    return Image.fromarray(np.clip(arr + grain, 0, 255).astype(np.uint8))

def blobs(seed, colors, rmin=0.22, rmax=0.42):
    rnd = random.Random(seed * 7 + 3)
    return [(c, (rnd.uniform(0.0, 1.0), rnd.uniform(0.0, 1.0), rnd.uniform(rmin, rmax))) for c in colors]

SETS = {
    "Aurora": [
        ("borealis", "#07111f", ["#1f6f8b", "#52e5a7", "#8a5cf6", "#0e3b5c"]),
        ("glacier",  "#0d1b2a", ["#1b4965", "#5fa8d3", "#cae9ff", "#274c77"]),
        ("nebula",   "#12002e", ["#5a189a", "#9d4edd", "#e0aaff", "#3c096c"]),
        ("lagoon",   "#03045e", ["#0077b6", "#00b4d8", "#90e0ef", "#023e8a"]),
    ],
    "Dusk": [
        ("ember",    "#2b0a0a", ["#9d0208", "#f48c06", "#ffba08", "#6a040f"]),
        ("peach",    "#3d1a4d", ["#c0587e", "#fc8f6c", "#ffd6a5", "#7a2e5f"]),
        ("citrus",   "#5f0f40", ["#e36414", "#fb8b24", "#ffd166", "#9a031e"]),
        ("rose",     "#2d0b2e", ["#7b2cbf", "#ff6b9a", "#ffc2d1", "#5a189a"]),
    ],
    "Studio": [
        ("graphite", "#15151c", ["#2a2a36", "#3d3d4d", "#22222c", "#30303c"]),
        ("slate",    "#1e1f29", ["#282a36", "#44475a", "#6272a4", "#343746"]),
        ("sand",     "#ece6d8", ["#d8cfbd", "#c2b59b", "#f6f1e7", "#ddd3c1"]),
        ("mist",     "#e4ebf4", ["#c5d3e8", "#a9bcd9", "#f2f6fb", "#d3dfee"]),
    ],
    "Neon": [
        ("dracula",  "#1e1f29", ["#bd93f9", "#ff79c6", "#8be9fd", "#6272a4"]),
        ("synth",    "#14001f", ["#ff2a6d", "#05d9e8", "#7700ff", "#d1f7ff"]),
        ("lime",     "#0b2a12", ["#2ec4b6", "#cbf3f0", "#80ed99", "#1b4332"]),
        ("tangerine","#2a0845", ["#ff4e00", "#ec9f05", "#f6d743", "#8e2de2"]),
    ],
}

for cat, items in SETS.items():
    os.makedirs(os.path.join(OUT, cat), exist_ok=True)
    for i, (name, base, colors) in enumerate(items):
        seed = abs(hash((cat, name))) % 10_000 if False else (sum(map(ord, cat + name)) * 31 + i)
        im = mesh(base, blobs(seed, colors), seed)
        path = os.path.join(OUT, cat, f"{name}.jpg")
        im.save(path, quality=88, optimize=True, progressive=True)
        print(" ", os.path.relpath(path))
