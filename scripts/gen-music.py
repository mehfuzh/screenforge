#!/usr/bin/env python3
"""Generate the bundled background-music library with Google's Lyria 2 (Vertex AI) and write
assets/background-audio/<category>/<Name>.mp3. Needs gcloud auth on the project below.

    python3 scripts/gen-music.py            # only generates tracks that are missing

New tracks are then made loopable by scripts/make-loops.py (needs numpy).
"""
import base64, json, os, subprocess, sys, urllib.error, urllib.request

PROJECT = os.environ.get("GCP_PROJECT", "smartloop-gcp-us-east")
URL = f"https://us-central1-aiplatform.googleapis.com/v1/projects/{PROJECT}/locations/us-central1/publishers/google/models/lyria-002:predict"
OUT = os.path.join(os.path.dirname(__file__), "..", "assets", "background-audio")
FFMPEG = os.environ.get("FFMPEG", "ffmpeg")
NEG = "vocals, singing, lyrics, harsh, distorted, abrupt ending, loud drops"
BED = "instrumental background music for a screen recording tutorial, unobtrusive, steady, loopable"

TRACKS = {
    "lo-fi": {
        "Warm Rhodes":   "warm lo-fi hip hop, soft rhodes chords, gentle swing drums, muted bass, vinyl texture, cozy, 82 bpm",
        "Late Coffee":   "mellow lo-fi chillhop, jazzy piano, brushed snare, round bass, relaxed and positive, 78 bpm",
        "City Window":   "lo-fi beat with vibraphone melody, soft kick, hi-hat shuffle, rainy city mood, calm, 86 bpm",
    },
    "electronic": {
        "Soft Pulse":    "minimal ambient electronica, glassy plucks, airy pads, soft four-on-the-floor kick, bright and focused, 118 bpm",
        "Neon Drive":    "light synthwave, warm analog arpeggios, gentle retro drums, uplifting, nostalgic, 104 bpm",
        "Clean Motion":  "modern tech product background, crisp percussion, plucked synth ostinato, clean bass, optimistic, 112 bpm",
    },
    "acoustic": {
        "Morning Folk":  "friendly acoustic folk bed, fingerpicked guitar, light piano, soft claps, warm and upbeat, 96 bpm",
        "Easy Ukulele":  "cheerful ukulele and glockenspiel, light shaker, playful and simple, 100 bpm",
        "Open Road":     "acoustic guitar strumming, soft strings, gentle kick, hopeful and calm, 90 bpm",
    },
    "cinematic": {
        "Bright Idea":   "uplifting corporate piano arpeggios, soft strings, light ticking percussion, motivating and clean, 110 bpm",
        "Calm Focus":    "calm ambient piano with soft string pad, slow evolving texture, thoughtful, 70 bpm",
        "New Horizon":   "inspiring documentary underscore, piano motif, warm strings, subtle pulse, elegant, 92 bpm",
    },
}

def token():
    return subprocess.check_output(["gcloud", "auth", "print-access-token"], text=True).strip()

def generate(prompt, tok):
    body = json.dumps({"instances": [{"prompt": f"{prompt}. {BED}", "negative_prompt": NEG}], "parameters": {"sample_count": 1}}).encode()
    req = urllib.request.Request(URL, data=body, headers={"Authorization": f"Bearer {tok}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=180) as r:
        d = json.load(r)
    p = d["predictions"][0]
    return base64.b64decode(p.get("bytesBase64Encoded") or p["audioContent"])

def main():
    tok = token()
    for cat, items in TRACKS.items():
        os.makedirs(os.path.join(OUT, cat), exist_ok=True)
        for name, prompt in items.items():
            mp3 = os.path.join(OUT, cat, f"{name}.mp3")
            if os.path.exists(mp3):
                continue
            wav = mp3 + ".wav"
            audio = None
            for attempt in (prompt, prompt.split(",")[0] + ", gentle instrumental, " + prompt.rsplit(",", 1)[-1].strip()):
                try:
                    audio = generate(attempt, tok)
                    break
                except urllib.error.HTTPError as e:
                    print(f"  {name}: rejected ({e.code}) {e.read().decode()[:200]}", file=sys.stderr)
            if audio is None:
                continue
            open(wav, "wb").write(audio)
            # loudness-normalise for a background bed, tiny edge fades, 192 kbps MP3
            subprocess.run([FFMPEG, "-v", "error", "-y", "-i", wav, "-af",
                            "loudnorm=I=-18:TP=-2:LRA=9,afade=t=in:d=0.05,areverse,afade=t=in:d=0.05,areverse",
                            "-ar", "48000", "-c:a", "libmp3lame", "-b:a", "192k", mp3], check=True)
            os.remove(wav)
            print(" ", os.path.relpath(mp3))
    # tracks are ~33 s and cut mid-bar: give each a beat-matched seamless loop
    subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "make-loops.py")], check=True)

if __name__ == "__main__":
    main()
