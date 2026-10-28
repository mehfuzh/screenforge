# ScreenForge license backend (Supabase + Stripe)

Recording and editing are free; **export needs a license**: a one-time US$49 lifetime key.
(A US$10/month subscription was offered until 14 October 2026 and never sold; `create-checkout` refuses
`plan: "monthly"`, `setup.sh` archives the Stripe price, and `stripe-webhook` would still renew a legacy subscription if one existed.)

Keys are Ed25519-signed tokens the app verifies **offline** against the public key embedded in `crates/sf-license` — the private key only ever lives here as a Supabase
secret, so the public GitHub repo exposes nothing sensitive.

```
 Buy button (app/site) ──▶ create-checkout ──▶ Stripe Checkout
 Stripe ──checkout.session.completed / invoice.paid──▶ stripe-webhook
     └─ sign key (SF_LICENSE_SIGNING_KEY) ─ store in `licenses` ─ email via Resend
 App "Already have a key?" ◀── user pastes the key from the email (verified offline)
 App "Refresh" (expired monthly key) ──▶ license-refresh ──▶ current key for that email
```

## Setup

`backend/setup.sh` performs every step below in one go (payment product and prices, webhook,
Supabase link, schema, functions, secrets; API keys are read from GCP Secret Manager). The
manual steps, for reference:

1. `cargo run -p sf-cli -- license keygen` → put `SF_LICENSE_SIGNING_KEY` in Supabase secrets and
   paste `ISSUER_PUBLIC_KEY_B64` into `crates/sf-license/src/lib.rs`.
2. Stripe: create a product with a one-time US$49 price (`setup.sh` does this; `update-price.sh <cents>` changes the price later, Stripe and Supabase in one go); set `STRIPE_SECRET_KEY`,
   `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_LIFETIME`.
3. Resend: `RESEND_API_KEY`, `LICENSE_FROM_EMAIL`.
4. `supabase db push && supabase functions deploy create-checkout stripe-webhook license-refresh license-activate free-export unsplash jamendo agent voice`
5. Point `PURCHASE_URL` in `crates/sf-license` at your checkout page (or the `create-checkout` URL).

## Activation from the app

The export dialog has no key field. The user enters the email used at checkout and presses
Activate. The app sends `{ email, device }` to `license-activate`, where `device` is a SHA-256 of
the machine's hardware UUID (`sf_license::device_id`; the raw UUID never leaves the machine).

- A license is active on **one computer at a time** (`license_activations`). That computer gets
  its key straight away, signed for it (`device` in the key payload): the app rejects a key bound
  to another computer, or to none, so a copied key file does not export elsewhere, even offline.
- Any other computer: the function emails a 6-digit code to the address on the license and
  returns `{ status: "code_sent" }`. The app asks for the code and sends `{ email, device, code }`;
  the function checks it, **moves the license** to that computer and returns its key.
- The app checks in quietly when the export dialog is created (`send: false`, never emails): a
  computer whose license moved away removes its key; offline, nothing changes.
- Codes are stored hashed (`license_codes`), expire after 15 minutes and allow 5 tries; an email
  gets at most one code a minute and 5 an hour (`license_code_sends`).
- A key is never returned for an email alone, and email lookups are exact: `%` and `_` cannot
  act as wildcards.

The key is verified offline and stored under `~/Library/Application Support/ScreenForge/`.
"Buy" calls `create-checkout` with the email and opens the Stripe URL. The same check-in renews an
expired monthly key. Keys from before device activation (and builds before 0.1.30, which only
trust the embedded public key offline) are not bound to a computer: new builds ask to activate
once; already-activated old builds keep working until updated, since they never ask the server. `license-refresh` only answers activated devices; builds before 0.1.30
that call it with an email alone are asked to update. Set `SUPABASE_URL` and
`SUPABASE_ANON_KEY` in `crates/sf-license/src/lib.rs` to the project's values (and deploy the
functions with `--no-verify-jwt`, or keep JWT verification: the anon key is sent as a bearer token).

Every export needs a license; there are no free exports since v0.1.40. Releases up to v0.1.39
allowed three without one and still call `free-export` (`{ device }` when the export dialog opens,
`{ device, record: true }` after an unlicensed export), so the function and the `free_exports`
table stay deployed until those versions are gone. Newer versions never call it.

## Unsplash backgrounds

Background → Unsplash searches photos through the `unsplash` function, so the Unsplash access
key stays on the server (`UNSPLASH_ACCESS_KEY`; create an app at unsplash.com/developers).

```
supabase secrets set UNSPLASH_ACCESS_KEY=<access key>
supabase db push
supabase functions deploy unsplash
```

`{ query, page }` returns a page of landscape results (an empty query lists the Wallpapers
topic). When a photo is picked the app sends `{ download: <download_location> }`, which
registers the download with Unsplash as its API guidelines require and returns the image URL;
the app saves the photo in the project's `assets/` (so it renders and exports offline) and
credits the photographer in the sidebar with referral links. Search replies are cached for every user in the
`unsplash_cache` table (migration `20261022000000_unsplash_cache.sql`): a search made in the last
6 hours is answered without calling Unsplash (normalised, so case and spacing do not matter),
a stale reply is served when Unsplash rate-limits or fails, and rows older than 7 days are
pruned. Download tracking is never cached. Until the function is deployed the
tab says the service is not available yet. A demo app on Unsplash allows 50 requests an hour;
apply for production access before release.

## Jamendo music

Audio → Find music on Jamendo searches Creative Commons tracks through the `jamendo` function,
so the client id stays on the server (`JAMENDO_CLIENT_ID`; register an app at
devportal.jamendo.com).

```
supabase secrets set JAMENDO_CLIENT_ID=<client id>
supabase db push
supabase functions deploy jamendo
```

`{ query, page, instrumental }` returns 20 tracks a page (an empty query lists the month's most
popular; `instrumental` defaults to true). Only tracks Jamendo allows to be downloaded are
returned. When a track is picked, the app downloads its MP3 straight from Jamendo's storage into
the project's `assets/` as `jamendo_<id>.mp3` and writes `jamendo_<id>.credit.json` beside it, so
the sidebar can show the artist and licence and copy a credit line for the video description.
Replies are cached in `jamendo_cache` (migration `20261025000000_jamendo_cache.sql`) the same way
as `unsplash_cache`.

Licences. Music set under a video is an Adaptation in every Creative Commons 3.0 licence
(§1(a): synchronising a musical work with a moving image, "synching"). So:

| Track licence | Under a video | Offered |
| --- | --- | --- |
| CC BY | yes, commercial too, with credit | always |
| CC BY-NC | only if the video earns nothing (no ads, sponsors, product promotion) | only with "Include non-commercial music" (never to the agent) |
| CC BY-SA, BY-NC-SA | the video would have to be released under CC BY-SA | never |
| CC BY-ND, BY-NC-ND | not allowed (no Adaptations) | never |
| none given | unknown | never |

The function asks Jamendo for `ccnd=false&ccsa=false` (and `ccnc=false` unless `noncommercial`)
and checks each track's `license_ccurl` again; the app checks once more before showing a track
and before downloading one. The credit line the app copies names the title, artist, track page
and licence with its link, which is what CC BY asks for.

Jamendo's API terms (devportal.jamendo.com/api_terms_of_use, §3.3, last changed 2013) allow the
API freely for non-commercial uses only; commercial use, "any use that is intended for or
directed toward commercial advantage or any monetary compensation", needs an agreement with
licensing@jamendo.com. ScreenForge is sold, so get that agreement before release. The same
section says applications must not be designed to cache content or offer offline access, while
the API also flags tracks the app may offer for download (`audiodownload_allowed`); confirm with
Jamendo that saving a chosen track into the user's project is covered. §4.1 asks for the artist
and Jamendo to be credited with a link from each track to its Jamendo page, which every result
row has.

Audio → Import from a link needs no backend: the app downloads the file itself, checks from its
first bytes that it is audio (MP3, M4A, AAC, WAV, OGG, FLAC or AIFF) and saves it in `assets/`.

## Agent (Claude)

The editor's Agent panel edits projects with Claude. The app runs the agent loop and its tools
(`crates/sf-agent`) and sends each Messages API request to the `agent` function as
`{ device, request }`; the function adds the Anthropic key, which only exists as a Supabase
secret, and returns Claude's message unchanged.

```
supabase secrets set ANTHROPIC_API_KEY=<key from the Claude Platform>
supabase db push            # agent_usage table (migration 20261022020000_agent_usage.sql)
supabase functions deploy agent
```

- Only what the agent needs is forwarded: the models the app offers (`MODELS`: Claude Sonnet
  5.5, which Auto always uses, Claude Opus 5.5 and Claude Haiku 5.5; kept in step with
  `crates/sf-agent/src/client.rs`). Credits are charged at each model's list price, so Opus
  uses them twice as fast as Sonnet and Haiku about 20 times slower; its own tools (no
  server tools such as web search), `max_tokens` up to 16,000. Sonnet and Opus requests get
  `fallbacks: "default"`; a reply from a fallback model is priced at that model's rates.
- Each reply's tokens are priced at list price and added to `agent_usage` for the device and
  UTC day. The anon key is public, so spending is capped until agent credits are sold:
  `AGENT_DEVICE_DAILY_USD` per device (default 0.50, then HTTP 402 with a message the app shows)
  and `AGENT_TOTAL_DAILY_USD` for everyone together (default 20); `0` turns a cap off (the
  per-device cap is off in production since 23 October 2026). Device ids are not secret,
  so the per-device cap alone does not stop abuse; the total cap is the backstop.
- The agent is part of Pro: the `agent` function refuses other computers (`pro_required`), and
  packs are sold only to Pro emails. `AGENT_STAFF_EMAILS` (comma separated) lets the team use it
  without Pro.
- Debug builds skip the backend: with `ANTHROPIC_API_KEY` in the environment or in a `.env`
  file (the repository's, which is git-ignored), the app calls the Claude API directly and the
  panel says "local key". Release builds always go through the backend.

## Signing in and out, and billing

The agent panel's credits menu shows the license email this computer is signed in with, and
offers **Switch account**, **Sign out** and, for Pro, **Manage subscription**:

- Signing in is the same emailed 6-digit code as Export → Activate (`license-activate`).
  Switching keeps the old account until the new one is confirmed, then releases it.
- `license-deactivate` `{ email, device }` removes this computer's activation: it stops using
  that account's credits and the license is free to activate elsewhere. The app deletes its key
  and email too (`sf_license::sign_out`).
- `billing-portal` `{ email, device }` → `{ url }` opens Stripe's customer portal (cancel at the
  end of the period, change the card, invoices) for a computer activated for that email only.
  It uses the account's default portal configuration (`PORTAL_RETURN_URL` sets where "Return"
  goes; default the pricing page).


## ElevenLabs voices

Narration (and the agent's `add_narration`) offers ElevenLabs voices beside the on-device Kokoro
ones. They are read through the `voice` function so the key stays on the server
(`ELEVENLABS_API_KEY`; elevenlabs.io → Developers → API keys, with Text to Speech access only).

```
supabase secrets set ELEVENLABS_API_KEY=<key>
supabase functions deploy voice
```

`{ device, voice, text, speed }` returns `{ audio, alignment, credits, sf_credits }`: 16-bit PCM
at 24 kHz (base64) and ElevenLabs' character timing, which the app turns into word timing for
captions. `voice` is the app's id (`el_brian` …); the ElevenLabs voice ids are kept in the
function (premade voices, so any workspace has them) and can change without an app release.
A take is at most 5,000 characters.

Credits. A reading is paid like an agent request, from the same balance (Pro's monthly credits,
then top-ups): it needs Pro (or staff / trial credits) and a positive balance, and costs
ElevenLabs' list price for its characters at 1 credit = US$0.01. `eleven_v4` is US$0.08 per 1,000
characters, so 8 credits per 1,000, about 7 credits for a minute of narration; 1,000 monthly
credits cover roughly two hours of narration if nothing else is spent. Packs keep the same
50% margin as for Claude. Set `ELEVENLABS_MODEL` / `ELEVENLABS_USD_PER_1K_CHARS` together to
change model (v4 Turbo: `eleven_v4_turbo`, 0.04). Its cost also counts toward the agent's
daily caps (`AGENT_DEVICE_DAILY_USD`, `AGENT_TOTAL_DAILY_USD`). The app keeps each voice's
preview in its cache folder, so a preview is charged once.

Licence. ElevenLabs' Free plan has no commercial licence; the workspace needs a paid plan
(Starter or above) before the voices are offered to customers, and enough characters or
usage-based billing for everyone's narration.
