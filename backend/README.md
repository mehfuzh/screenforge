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
4. `supabase db push && supabase functions deploy create-checkout stripe-webhook license-refresh free-export unsplash`
5. Point `PURCHASE_URL` in `crates/sf-license` at your checkout page (or the `create-checkout` URL).

## Activation from the app

The export dialog has no key field. The user enters the email used at checkout and presses
Activate: the app calls `license-refresh` with `{ email }`, verifies the returned key offline
and stores it under `~/Library/Application Support/ScreenForge/`. "Buy"
calls `create-checkout` with the email and opens the Stripe URL. An expired monthly key is
refreshed automatically the next time the dialog opens. Set `SUPABASE_URL` and
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
