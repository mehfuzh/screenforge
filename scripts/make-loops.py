#!/usr/bin/env python3
"""Make the bundled background-music tracks loop seamlessly. The tracks are cut mid-bar, so a
plain wrap restarts the music off the beat. This finds the point in the last third whose rhythm
best matches the opening (onset-envelope correlation, then sample-level alignment), ends the
loop there and crossfades (equal power) the music that follows it into the first FADE seconds:
when playback or export wraps around, the beat carries on.

    python3 scripts/make-loops.py                     # every track not processed yet
    python3 scripts/make-loops.py path/to/track.mp3   # specific files

Processed files carry the tag comment=seamless-loop, so running it again is a no-op.
"""
import glob, os, subprocess, sys

import numpy as np

FFMPEG = os.environ.get("FFMPEG", "ffmpeg")
FFPROBE = os.environ.get("FFPROBE", "ffprobe")
RATE, CH, FADE = 48000, 2, 2.5
TAG = "seamless-loop"
ROOT = os.path.join(os.path.dirname(__file__), "..", "assets", "background-audio")


def is_done(path):
    out = subprocess.run([FFPROBE, "-v", "error", "-show_entries", "format_tags=comment", "-of", "csv=p=0", path], capture_output=True, text=True).stdout
    return TAG in out


def best_loop_point(a, x):
    """Sample index in the last third whose following music best matches the opening."""
    mono = a.mean(1)
    hop = 480  # 10 ms
    n = len(mono) // hop
    env = np.sqrt((mono[: n * hop].reshape(n, hop) ** 2).mean(1))
    onset = np.maximum(np.diff(env, prepend=env[0]), 0.0)  # rises = note/drum onsets
    win = int(6.0 * RATE / hop)  # compare 6 s of rhythm
    ref = onset[:win] - onset[:win].mean()
    lo = int(len(mono) * 0.7 / hop)
    hi = (len(mono) - x) // hop - 1
    best, best_score = hi, -1e9
    for i in range(lo, hi):
        seg = onset[i:i + win]
        if len(seg) < win // 2:
            break
        r = ref[: len(seg)]
        seg = seg - seg.mean()
        score = float((r * seg).sum() / (np.linalg.norm(r) * np.linalg.norm(seg) + 1e-9))
        score += 0.08 * (i - lo) / max(hi - lo, 1)  # keep as much of the track as possible
        if score > best_score:
            best, best_score = i, score
    coarse = best * hop
    # refine to the sample: align waveforms within ±10 ms
    span = 480
    ref_w = mono[:4800]
    best_off, best_c = 0, -1e9
    for off in range(-span, span + 1, 4):
        s0 = coarse + off
        if s0 < 0 or s0 + 4800 > len(mono):
            continue
        c = float((ref_w * mono[s0:s0 + 4800]).sum())
        if c > best_c:
            best_off, best_c = off, c
    print(f"    loop point {(coarse + best_off) / RATE:.2f}s (rhythm match {best_score:.2f})")
    return coarse + best_off


def make_loop(path):
    raw = subprocess.run([FFMPEG, "-v", "error", "-nostdin", "-i", path, "-f", "f32le", "-ac", str(CH), "-ar", str(RATE), "-"], capture_output=True, check=True).stdout
    a = np.frombuffer(raw, dtype=np.float32).reshape(-1, CH).copy()
    x = int(FADE * RATE)
    if len(a) < 4 * x:
        print(f"  skip (too short): {path}")
        return
    end = best_loop_point(a, x)
    t = np.linspace(0.0, np.pi / 2, x, dtype=np.float32)[:, None]
    head, tail = a[:x], a[end:end + x]
    loop = a[:end].copy()
    # the loop starts with what followed the loop point fading out over the opening fading in:
    # playing past loop[-1] continues seamlessly into loop[0]
    loop[:x] = head * np.sin(t) + tail * np.cos(t)
    peak = float(np.abs(loop).max())
    if peak > 0.98:
        loop *= 0.98 / peak
    tmp = path + ".loop.mp3"
    subprocess.run([FFMPEG, "-v", "error", "-nostdin", "-y", "-f", "f32le", "-ac", str(CH), "-ar", str(RATE), "-i", "-",
                    "-c:a", "libmp3lame", "-b:a", "192k", "-metadata", f"comment={TAG}", tmp], input=loop.tobytes(), check=True)
    os.replace(tmp, path)
    print(f"  {os.path.relpath(path)}: {len(a) / RATE:.2f}s -> {len(loop) / RATE:.2f}s loop")


def main():
    paths = sys.argv[1:] or sorted(glob.glob(os.path.join(ROOT, "*", "*.mp3")))
    for p in paths:
        if is_done(p):
            continue
        make_loop(p)


if __name__ == "__main__":
    main()
