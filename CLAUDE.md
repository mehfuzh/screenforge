# ScreenForge — project instructions for Claude Code

- Commit messages and PR descriptions: never add `Co-Authored-By: Claude …`,
  `Generated with Claude Code` or any other AI attribution line. Plain messages only.
- Author identity for commits: Mehfuz Hossain <mehfuz@smartloop.ai>.
- Run `cargo test --workspace` before committing; rebuild the app bundle with `scripts/bundle.sh`.
- Format and pipeline notes are kept privately in `~/Library/Application Support/ScreenForge/private-notes`
  (not in the repo). The user's real projects are read-only: copy before editing.

## Design guidelines (toolbar and HUD)

- Buttons and pickers on the floating toolbar use square-ish corners: corner radius 2 px at
  most. The bar itself and the recording HUD use 10 px corners; no pill shapes anywhere.
- Icons are painted vectors (`widgets::Icon`), never emoji or font glyphs.
- Colours come from `theme::p()`; no hard-coded `Color32` values in UI code.
