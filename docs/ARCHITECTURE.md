# Architecture

ScreenForge uses the `.screenstudio` package format so projects stay interchangeable, but the
implementation is native Rust: no Electron, no WebCodecs, no TypeGPU.

```
                 ┌──────────────┐        ┌──────────────┐
  record ──────▶ │ sf-recording │ ─────▶ │  .screenstudio package (recording/, project.json) │
                 └──────────────┘        └──────────────┘
                                                │ open
                                                ▼
   ┌────────────┐   ┌───────────┐   ┌────────────────┐   ┌───────────┐   ┌───────────┐
   │ sf-project │ ▶ │ sf-engine │ ▶ │   sf-session   │ ▶ │ sf-render │ ▶ │ sf-export │
   │ schema/IO  │   │ time/anim │   │ sources+frames │   │ wgpu      │   │ ffmpeg    │
   └────────────┘   └───────────┘   └────────────────┘   └───────────┘   └───────────┘
                                                ▲ egui preview texture
                                          apps/studio (editor UI)
```

## Time

* **Source time**: ms into the recording session. Everything stored on disk (zooms, layouts,
  masks, voice-overs, input events) is in source time.
* **Output (playback) time**: ms on the edited timeline. `sf_engine::timeline::Timeline` lays the
  slices end to end: `playback = Σ sliceDuration × timeScale` (`timeScale 0.5` = 2× speed).
* Springs are stepped with `dt × speed` so a sped-up clip animates the same way.

## Geometry (the stage)

`sf_engine::stage::StageGeometry`: the canvas ("stage") is 1000 units tall; its width comes
from `output.aspectRatio` (`auto` → 16:9 for wide recordings, otherwise a canvas hugging the
recording). Padding is a ratio of the screen height. The screen is fitted inside the padded
"zoom area"; a zoom scales the screen rect by `zoom` around the focus point and clamps it so
the zoom area is always covered. The rect `(x, y, w, h)` is animated by `screenMovementSpring`.

## Zooms and cursor

* Automatic zooms: each click opens `[t−300, t+2500]`, merged when closer than 2.5 s.
* Inside an automatic zoom the camera looks at *click groups*: cursor samples are grouped until
  their bounding box exceeds half the visible area; the group centre is time-weighted.
* Cursor: spike shake removal, 10 ms decimation, spring smoothing (null = raw), snap to click
  within 500 ms, pressed scale 0.9, horizontal-velocity tilt (12°/(px/ms), ±20°, τ=120 ms).

## Rendering

`sf-render` draws every layer as a rounded quad with one WGSL shader (`composite.wgsl`):
background (colour / gradient / cover-fit image), soft shadow (blurred SDF), screen with
rounded corners and inset, camera overlay, cursor sprite (rotated around its hotspot).
The target texture is read back for export or sampled by egui for the preview.

## Media

`sf-media` shells out to ffmpeg (bundled or on PATH):
* read: `-i playlist.m3u8 … -vf fps=60,scale=… -f rawvideo -pix_fmt rgba -` (recordings are VFR;
  we resample to CFR so frame index × 1/fps is the timestamp)
* write: `-f rawvideo … -i pipe:0 -c:v h264_videotoolbox|libx264 …` with `-filter_complex`
  `adelay/atempo/volume/amix` for audio mixing per slice
* audio preview: `-f f32le` decode to memory, played with cpal

## Recording

`sf-recording` reproduces `polyrecorder`'s output: ScreenCaptureKit frames → constant 60 fps
repeater → ffmpeg HLS fMP4 muxer (`channel-1-display-0.m3u8` + init + `.m4s`), CGEventTap for
mouse/keyboard JSON logs, NSCursor polling for `cursors.json` + PNG sprites, `metadata.json`.

## Compatibility rules

* Unknown JSON fields are preserved (`extra` maps) so files written by newer apps round-trip.
* The writer records a compatible format version in `meta.json`, so other apps that read the
  package format open saved projects.
* Integral floats are written as integers (JavaScript `JSON.stringify` style).
