#!/usr/bin/env python3
"""Generate the bundled camera looks as DaVinci Resolve style 3D LUTs (`assets/luts/*.cube`).

Each look is a smooth function on sRGB 0..1 pixels, sampled on a 17³ grid in the Resolve/Iridas
`.cube` layout (red varies fastest). Run from the repo root: `python3 scripts/gen-luts.py`.
"""
import os
import numpy as np

SIZE = 17
OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "luts")
LUMA = np.array([0.2126, 0.7152, 0.0722])


def luma(c):
    return (c * LUMA).sum(axis=-1, keepdims=True)


def saturate(c, k):
    y = luma(c)
    return y + (c - y) * k


def contrast(c, k, pivot=0.435):
    return (c - pivot) * k + pivot


def scurve(c, k):
    # smooth S around 0.5; k = 0 identity
    return c + k * (c - 0.5) * (1 - np.abs(c - 0.5) * 2) * 0.5


def lift_gamma_gain(c, lift=(0, 0, 0), gamma=(1, 1, 1), gain=(1, 1, 1)):
    lift, gamma, gain = map(np.asarray, (lift, gamma, gain))
    c = c + lift * (1 - c)
    c = np.power(np.clip(c, 0, 1), 1.0 / gamma)
    return c * gain


def split_tone(c, shadow_rgb, highlight_rgb, amount):
    y = luma(c)
    sh = np.asarray(shadow_rgb) * (1 - y) ** 2
    hi = np.asarray(highlight_rgb) * y ** 2
    return c + amount * (sh + hi)


def teal_orange(c):
    c = scurve(c, 0.25)
    c = split_tone(c, (-0.06, 0.02, 0.08), (0.08, 0.02, -0.08), 1.0)
    return saturate(c, 1.12)


def warm_film(c):
    c = lift_gamma_gain(c, lift=(0.035, 0.03, 0.02), gamma=(1.03, 1.0, 0.97), gain=(1.0, 0.98, 0.93))
    # gentle highlight roll-off
    c = np.where(c > 0.8, 0.8 + (c - 0.8) * 0.75, c)
    return saturate(c, 0.95)


def cool_matte(c):
    c = lift_gamma_gain(c, lift=(0.05, 0.055, 0.07), gamma=(0.98, 1.0, 1.04), gain=(0.94, 0.97, 1.0))
    c = contrast(c, 0.92)
    return saturate(c, 0.85)


def vintage(c):
    c = lift_gamma_gain(c, lift=(0.07, 0.06, 0.03), gain=(0.96, 0.95, 0.88))
    c = split_tone(c, (0.0, 0.03, -0.02), (0.05, 0.03, -0.04), 1.0)
    c = contrast(c, 0.9)
    return saturate(c, 0.8)


def black_white(c):
    # orthochromatic-ish mix (a touch of red filter), then contrast
    y = (c * np.array([0.35, 0.55, 0.10])).sum(axis=-1, keepdims=True)
    y = scurve(y, 0.3)
    return np.repeat(y, 3, axis=-1)


def bleach_bypass(c):
    y = luma(c)
    desat = y + (c - y) * 0.45
    # overlay blend of the picture with its own luma
    over = np.where(y < 0.5, 2 * desat * y, 1 - 2 * (1 - desat) * (1 - y))
    return desat * 0.4 + over * 0.6


def punchy(c):
    c = contrast(c, 1.18)
    return saturate(c, 1.25)


LOOKS = [
    ("Teal & Orange", teal_orange),
    ("Warm Film", warm_film),
    ("Cool Matte", cool_matte),
    ("Vintage", vintage),
    ("Black & White", black_white),
    ("Bleach Bypass", bleach_bypass),
    ("Punchy", punchy),
]


def grid(n):
    r, g, b = np.meshgrid(np.linspace(0, 1, n), np.linspace(0, 1, n), np.linspace(0, 1, n), indexing="ij")
    # red fastest: iterate b (slowest), g, r
    return np.stack([r, g, b], axis=-1).transpose(2, 1, 0, 3).reshape(-1, 3)


def main():
    os.makedirs(OUT, exist_ok=True)
    pts = grid(SIZE)
    for name, fn in LOOKS:
        out = np.clip(fn(pts.copy()), 0, 1)
        path = os.path.join(OUT, f"{name}.cube")
        with open(path, "w") as f:
            f.write(f'TITLE "{name}"\nLUT_3D_SIZE {SIZE}\n')
            for rgb in out:
                f.write("%.6f %.6f %.6f\n" % tuple(rgb))
        print(path, os.path.getsize(path), "bytes")


if __name__ == "__main__":
    main()
