# Status (2026-10-12)

## Works
- Saves `.screenforge` projects (v4 schema) and presets (`.screenforgepreset`; `.screenstudiopreset` presets are listed too).
- Imports v3 and v4 `.screenstudio` projects (v3 is migrated on open): opening one copies it into `~/ScreenForge Projects` as a `.screenforge` package; `.screenstudio` packages already in that folder (written by ScreenForge ≤ 0.1.13) are renamed in place. `sf import` does the same from the command line.
- Editor: title bar, preview toolbar, tool rail, sidebar pages
  (General, Cursor, Camera Layout, Camera Style, Captions, Audio, Shortcuts, Animations, Device),
  transport, timeline with clips (trim, split, speed, context menus), zooms (add, move, resize,
  auto zooms), **independent audio track** (detach, split, move, trim, mute — a ScreenForge
  extension stored as `sfAudioClips`), undo/redo, audio preview playback.
- Rendering: stage geometry, zoom camera that follows the cursor, cursor smoothing/tilt, backgrounds
  (built-in wallpapers, the Mac's own wallpapers, or `SF_ASSETS`), shadow, inset,
  camera layouts (overlay/split/fullscreen), masks, click effects.
- Captions: on-device Whisper transcription of the microphone (`sf-transcribe`; base/small
  models downloaded on first use from the `models-v1` GitHub release, Hugging Face as a
  fallback, SHA-256 verified), saved as `transcripts/microphone-session-N.json` like Screen
  Studio; rendered into preview and export (font, size, colour, position, line-by-line or
  word-by-word with appear/fade/slide); SRT export that follows cuts and speed
  (`sf transcribe`, `sf srt`, or the Captions panel).
- Keyboard shortcut overlay: key caps (`⇧ ⌘ S`) for shortcuts pressed while recording, near the bottom of the video in preview and export (`captions.enableShortcuts`, size, single keys); key symbols come from an Inter subset (`Inter-SemiBold-symbols.ttf`). New recordings also store the macOS `keyCode` of each key event.
- Crop: the Crop tool in the preview toolbar shows the whole recording with a draggable frame (move, eight resize handles, ⏎ apply, ⎋ cancel) and writes `config.crop.rect01`.
- Export: MP4 (H.264/HEVC), ProRes, GIF, WebM with mixed audio and quality presets.
- Recording: picker toolbar (Display/Window/Area/Device, camera/mic/system audio pickers,
  countdown), ScreenCaptureKit recorder writing the package's `recording/` layout
  (HLS fMP4, cursors, input logs, metadata), project creation with automatic zooms.
- Webcam recording: the camera picker records `channel-N-camera-S.m3u8` (H.264 HLS, 30 fps, up to
  1280 px wide) through ffmpeg's AVFoundation (macOS) or V4L2 (Linux) input, in step with the
  display sessions; new projects show it with the camera-overlay layout.

## Not yet
- Live camera preview while recording; window picking / area selection UI (area records a centred 60 % region); iPhone/iPad.
- Editing the transcript text, Apple Speech transcription, device frame compositing, color grading/LUTs, cutout matting, face tracking, motion blur.
- Crop aspect presets, Unsplash, background audio library playback, click sounds in preview/export.
- More bundled assets: cursor sets and device frames.

## Appearance

- Dark and light palettes (`apps/studio/src/theme.rs`); follows the system appearance by default, with a Match system / Light / Dark choice in the picker's settings menu (stored in prefs as `appearance`).
- Editor window uses a transparent full-size title bar so the traffic lights sit inside the toolbar.
- A project can be opened from the command line: `screenforge My.screenforge`.
- Projects open from Finder (double-click, Open With, drop on the Dock icon), at launch or while the app runs: `apps/studio/src/open_docs.rs` handles the `odoc` Apple Event. The Open project dialog starts in `~/ScreenForge Projects`.

## Website

- `site/index.html` is the landing page. `scripts/deploy-site.sh` publishes it to Cloudflare Pages under the custom domain (default screenforge.studio) and `.github/workflows/site.yml` redeploys on push once the `CLOUDFLARE_API_TOKEN` / `CLOUDFLARE_ACCOUNT_ID` secrets exist. GitHub Pages is not used: it is unavailable for private repos on the free plan.

## Roadmap items (disabled in the UI)

- Masks and highlights: blur or hide regions, spotlight areas
- Device frames: iPhone/iPad recording and mockup frames
- Unsplash backgrounds: online wallpaper search
- Face tracking and color grading: camera centring, looks and LUTs
