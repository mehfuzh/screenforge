# Status (2026-10-23)

## Works
- AI editing agent (`sf-agent`, Agent page of the sidebar, ⌘L; the page a project opens on): the user asks in plain words, or picks Auto edit for a first cut, and Claude edits the project through tools covering what can be done by hand: cut, speed, split, zooms, masks, camera layouts, audio clips, music and voice-overs, narration (`add_narration`), every sidebar setting (`patch_project`), presets, Unsplash backgrounds, importing files, transcribing and opening export. It reads the transcript, finds typing, clicks and idle time in the input log (`get_activity`), measures motion in the video itself for recordings without a log (`get_motion`), and looks at stills (`view_frames`). Edits keep the editor's lane rules, show on the timeline while it works, and a whole answer is one undo; editing by hand stops the turn. Models: Sonnet 5.5 (Auto), Opus 5.5, Haiku 5.5. Requests go through the Supabase `agent` function (Pro only; `AGENT_STAFF_EMAILS` for the team); debug builds use a local `ANTHROPIC_API_KEY` directly.
- Pro and credits: US$20/month with export and 1,000 agent credits per billing period; packs of 500 / 1,250 / 2,500 credits (US$10 / 25 / 50) that never expire; 1 credit = US$0.01 of Claude usage at list price. Credits menu in the agent panel (balance, plan, top-ups, account: sign in with an emailed code, switch account, sign out, manage subscription). See `backend/README.md`.
- AI voice narration (`sf-tts`): Kokoro-82M through ONNX Runtime with misaki's pronunciation dictionaries, three male and three female voices, downloaded once (about 335 MB, SHA-256 verified). The Narration panel reads a script into a voice-over take at the playhead; a narrated take can change its voice or script in place, and "Read all" re-reads every take in another voice. Narrated takes carry their words as their transcript.
- ElevenLabs voices (`sf_tts::cloud`, backend `voice` function): the Narration panel switches engine (On this computer | ElevenLabs), then shows that engine's voices; eight premade voices on `eleven_v4`, read online with character timing turned into word timing for captions. Paid from the agent credits (8 per 1,000 characters at list price), Pro only; the ElevenLabs key stays on the server. Previews are cached per voice and speed. The agent's `add_narration` offers them and picks Eric (`el_eric`) for studio quality or a more realistic voice.
- Voice-overs: record over the video from a movable HUD with a microphone picker and a "Too loud" warning; a per-item Boost (Off / +6 / +12 / +18 dB, soft-knee compressor then limiter) for audio clips and takes (`sfBoostDb`).
- Video import: a `.mov`, `.mp4` or `.m4v` dropped on the window, passed on the command line or chosen from New becomes a project (`sf_recording::import`); dropped on an open project it is appended after the last clip. Tall phone videos start in a phone frame.
- Saves `.screenforge` projects (v4 schema) and presets (`.screenforgepreset`; `.screenstudiopreset` presets are listed too). The Presets menu lists them as rows (click applies; pencil renames, trash deletes after a confirmation) and "Save preset as…" asks for a name; names must be unique in `~/Documents/ScreenForge Presets`.
- Imports v3 and v4 `.screenstudio` projects (v3 is migrated on open): opening one copies it into `~/ScreenForge Projects` as a `.screenforge` package; `.screenstudio` packages already in that folder (written by ScreenForge ≤ 0.1.13) are renamed in place. `sf import` does the same from the command line.
- Editor: title bar, preview toolbar, tool rail, sidebar pages
  (General, Cursor, Camera Layout, Camera Style, Captions, Audio, Shortcuts, Animations, Device),
  transport, timeline with clips (trim, split, speed, context menus), zooms (add, move, resize,
  auto zooms), **independent audio track** (detach, split, move, trim, mute — a ScreenForge
  extension stored as `sfAudioClips`), undo/redo, audio preview playback.
- Rendering: stage geometry, zoom camera that follows the cursor, cursor smoothing/tilt, backgrounds
  (built-in wallpapers, the Mac's own wallpapers, Unsplash photo search, or `SF_ASSETS`), shadow, inset,
  camera layouts (overlay/split/fullscreen), masks, click effects.
- Captions: on-device Whisper transcription of the microphone (`sf-transcribe`; base/small
  models downloaded on first use from the `models-v1` GitHub release, Hugging Face as a
  fallback, SHA-256 verified), saved as `transcripts/microphone-session-N.json` like Screen
  Studio; rendered into preview and export (font, size, colour, position, line-by-line or
  word-by-word with appear/fade/slide); recorded voice-overs are transcribed too (`transcripts/voice-over-<stem>.json`), and their words follow each take when it is moved, trimmed or split; SRT and WebVTT export that follow cuts and speed
  (`sf transcribe`, `sf srt`, or the Captions panel).
- Keyboard shortcut overlay: key caps (`⇧ ⌘ S`) for shortcuts pressed while recording, near the bottom of the video in preview and export (`captions.enableShortcuts`, size, single keys); key symbols come from an Inter subset (`Inter-SemiBold-symbols.ttf`). New recordings also store the macOS `keyCode` of each key event.
- Crop: the Crop tool in the preview toolbar shows the whole recording with a draggable frame (move, eight resize handles, ⏎ apply, ⎋ cancel) and writes `config.crop.rect01`.
- Masks: the Mask button on the tool rail adds a blur mask at the playhead. Masks get their own timeline lane (move, resize, Blur / Highlight, disable, remove), an inspector (blur amount or dimming, several areas per mask), and their areas are moved and resized on the preview while the playhead is inside the mask.
- Update available: on launch (at most daily, `prefs.check_updates`) or on request the app asks GitHub's releases API for the newest version; a dot on the toolbar's gear, an "Update to vX…" menu item and an editor title-bar button open the release page.
- Export: MP4 (H.264/HEVC), ProRes, GIF, WebM with mixed audio and quality presets.
- Recording: picker toolbar (Display/Window/Area/Device — Device records an iPhone or iPad connected by cable through AVFoundation, `sf-recording::device`, camera/mic/system audio pickers,
  window picking, area selection by dragging, a live camera preview bubble while recording, countdown), ScreenCaptureKit recorder writing the package's `recording/` layout
  (HLS fMP4, cursors, input logs, metadata), project creation with automatic zooms.
- Camera picture (Camera Style page): face tracking keeps the face centred in the webcam window
  (`camera.enableFaceTracking`, framing `sfFaceTrackingZoom`; `sf-vision` runs the SeetaFace
  cascade on the decoded frame every 200 ms of source time, a critically damped spring glides
  the crop window with a dead zone for small movements, no face → whole picture). Colour grading
  follows DaVinci Resolve's Primaries palette: Lift / Gamma / Gain / Offset colour wheels with
  master dials, Temperature, Tint, Contrast + Pivot (0.435), Color Boost, Shadows, Highlights,
  Saturation and Hue (50 = unity), stored in Resolve's display units as `camera.sfColorGrade`;
  3D LUTs in `.cube` format (`camera.lut` + `lutIntensity`: bundled looks under `assets/luts/`,
  generated by `scripts/gen-luts.py`, or a file imported into the project's `assets/`); the
  existing `sharpen01` and `grain` sliders now render too. Everything runs in the camera draw
  of `composite.wgsl` (mode 1 with `grade`), in preview and export.
- Webcam recording: the camera picker records `channel-N-camera-S.m3u8` (H.264 HLS, 30 fps, up to
  1280 px wide) through ffmpeg's AVFoundation (macOS), V4L2 (Linux) or DirectShow (Windows) input, in step with the
  display sessions; new projects show it with the camera-overlay layout.

## Not yet
- Editing the transcript text, Apple Speech transcription, cutout matting.
- Motion blur and click sounds: their settings are in the sidebar (and the agent's settings map) but preview and export do not render them yet.
- Appending a video to a recording that was paused and resumed.
- Colour grading for the screen recording itself (the grade applies to the camera picture), Resolve's log wheels / curves / qualifiers, `.cube` LUTs with a non-unit `DOMAIN_MIN/MAX` (parsed, but the domain is not remapped yet).
- Crop aspect presets.
- Device frames are drawn shapes (`sf-engine::device`: phone, punch-hole phone, tablet in four finishes), not bundled images. (Cursor sets are drawn in `sf-session::cursor_sets`: Classic, Light, Pixel, Dot and Hand besides the recorded pointer.)

## Appearance

- Dark and light palettes (`apps/studio/src/theme.rs`); ScreenForge Dark by default, with ScreenForge Light / Match system / Catppuccin Mocha / Solarized Light / Osaka Jade in the picker's settings menu (stored in prefs as `appearance`). The website (`site/*.html`) follows the system appearance, with a light/dark toggle stored in localStorage.
- Editor window uses a transparent full-size title bar so the traffic lights sit inside the toolbar.
- A project can be opened from the command line: `screenforge My.screenforge`.
- Projects open from Finder (double-click, Open With, drop on the Dock icon), at launch or while the app runs: `apps/studio/src/open_docs.rs` handles the `odoc` Apple Event. The Open project dialog starts in `~/ScreenForge Projects`.

## Website

- `site/index.html` is the landing page. `scripts/deploy-site.sh` publishes it to Cloudflare Pages under the custom domain (default screenforge.studio) and `.github/workflows/site.yml` redeploys on push once the `CLOUDFLARE_API_TOKEN` / `CLOUDFLARE_ACCOUNT_ID` secrets exist. GitHub Pages is not used: it is unavailable for private repos on the free plan.

## Roadmap items (disabled in the UI)

None: everything that was greyed out (masks, crop, shortcut overlay, device frames, Unsplash backgrounds) has shipped.
