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
4. `supabase db push && supabase functions deploy create-checkout stripe-webhook license-refresh license-activate free-export unsplash agent`
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

The first three exports work without any license (`sf_license::FREE_EXPORTS`). The count is
kept in a file next to the key **and** in the `free_exports` table, keyed by a SHA-256 of the
machine's hardware UUID (`IOPlatformUUID`; the raw UUID never leaves the Mac). The app calls
`free-export` with `{ device }` when the export dialog opens and `{ device, record: true }` after
each unlicensed export, and takes the higher of the local and server counts, so deleting the app
or its support folder does not restart the free exports. Offline, the local file alone counts.

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

