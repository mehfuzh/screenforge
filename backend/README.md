# ScreenForge license backend (Supabase + Stripe)

Recording and editing are free; **export needs a license** (US$10/month subscription, or a
one-time lifetime key). Keys are Ed25519-signed tokens the app verifies **offline** against the
public key embedded in `crates/sf-license` — the private key only ever lives here as a Supabase
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
2. Stripe: create a product with a `$10/month` recurring price (and optionally a lifetime price);
   set `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `STRIPE_PRICE_MONTHLY`, `STRIPE_PRICE_LIFETIME`.
3. Resend: `RESEND_API_KEY`, `LICENSE_FROM_EMAIL`.
4. `supabase db push && supabase functions deploy create-checkout stripe-webhook license-refresh`
5. Point `PURCHASE_URL` in `crates/sf-license` at your checkout page (or the `create-checkout` URL).

## Activation from the app

The export dialog has no key field. The user enters the email used at checkout and presses
Activate: the app calls `license-refresh` with `{ email }`, verifies the returned key offline
and stores it under `~/Library/Application Support/ScreenForge/`. "Buy monthly" / "Buy lifetime"
call `create-checkout` with the email and open the Stripe URL. An expired monthly key is
refreshed automatically the next time the dialog opens. Set `SUPABASE_URL` and
`SUPABASE_ANON_KEY` in `crates/sf-license/src/lib.rs` to the project's values (and deploy the
functions with `--no-verify-jwt`, or keep JWT verification: the anon key is sent as a bearer token).
The first three exports work without any license (`sf_license::FREE_EXPORTS`).
