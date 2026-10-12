# Status (2026-10-12)

## Works
- Opens v3 and v4 `.screenstudio` projects (v3 is migrated on open) and saves v4; presets (`.screenstudiopreset`).
- Editor: title bar, preview toolbar, tool rail, sidebar pages
  (General, Cursor, Camera Layout, Camera Style, Captions, Audio, Shortcuts, Animations, Device),
  transport, timeline with clips (trim, split, speed, context menus), zooms (add, move, resize,
  auto zooms), **independent audio track** (detach, split, move, trim, mute — a ScreenForge
  extension stored as `sfAudioClips`), undo/redo, audio preview playback.
- Rendering: stage geometry, zoom camera with click groups, cursor smoothing/tilt, backgrounds
  (built-in wallpapers, the Mac's own wallpapers, or `SF_ASSETS`), shadow, inset,
  camera layouts (overlay/split/fullscreen), masks, click effects.
- Export: MP4 (H.264/HEVC), ProRes, GIF, WebM with mixed audio and quality presets.
- Recording: picker toolbar (Display/Window/Area/Device, camera/mic/system audio pickers,
  countdown), ScreenCaptureKit recorder writing the package's `recording/` layout
  (HLS fMP4, cursors, input logs, metadata), project creation with automatic zooms.

## Not yet
- Webcam capture during recording; window picking / area selection UI (area records a centred 60 % region); iPhone/iPad.
- Captions/transcription, keystroke overlay, device frame compositing, color grading/LUTs, cutout matting, face tracking, motion blur.
- Crop editing UI, Unsplash, background audio library playback, click sounds in preview/export.
- More bundled assets: cursor sets and device frames.

## Appearance

- Dark and light palettes (`apps/studio/src/theme.rs`); follows the system appearance by default, with a Match system / Light / Dark choice in the picker's settings menu (stored in prefs as `appearance`).
- Editor window uses a transparent full-size title bar so the traffic lights sit inside the toolbar.
- A project can be opened from the command line: `screenforge My.screenstudio`.

## Website

- `site/index.html` is the landing page. `scripts/deploy-site.sh` publishes it to Cloudflare Pages under the custom domain (default screenforge.studio) and `.github/workflows/site.yml` redeploys on push once the `CLOUDFLARE_API_TOKEN` / `CLOUDFLARE_ACCOUNT_ID` secrets exist. GitHub Pages is not used: it is unavailable for private repos on the free plan.

## Roadmap items (disabled in the UI)

- Camera recording: webcam capture and the camera layouts/styles that depend on it
- Masks and highlights: blur or hide regions, spotlight areas
- Crop editing: the Crop tool in the preview toolbar
- Captions: transcription and caption rendering
- Keyboard shortcut overlay: key-cap rendering of recorded keystrokes
- Device frames: iPhone/iPad recording and mockup frames
- Unsplash backgrounds: online wallpaper search
- Face tracking and color grading: camera centring, looks and LUTs
- Remove project: move a project to the Trash from the editor
- Licensing backend: hosted key issuing (Supabase + Stripe) so Export can be unlocked
