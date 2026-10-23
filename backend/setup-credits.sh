#!/usr/bin/env bash
# Pro (US$20/month: export + 1,000 agent credits a month) and agent credit packs, end to end:
#   Stripe (live): product "ScreenForge Pro" with a monthly price, product "ScreenForge agent
#                  credits" with one price per pack (US$10 / 25 / 50 = 500 / 1,250 / 2,500
#                  credits, a 50% margin over Claude list prices); the webhook gets the events
#                  credits need.
#   Supabase:      price ids as secrets, the agent_credits migration, and the agent, credits,
#                  create-checkout and stripe-webhook functions.
#
# Idempotent (prices are found by lookup key). Needs: stripe CLI (logged in to the Smartloop
# account), supabase CLI (logged in, project linked by setup.sh), jq.
set -euo pipefail
cd "$(dirname "$0")"

REF=${SUPABASE_PROJECT_REF:-kxpppjdcqkxpybdawxum}
for t in stripe supabase jq; do command -v $t >/dev/null || { echo "missing: $t"; exit 1; }; done

product() { # name description → id (created when missing)
  local id
  id=$(stripe products list --live --limit 100 | jq -r --arg n "$1" '.data[] | select(.name == $n and .active) | .id' | head -1)
  if [ -z "$id" ]; then
    id=$(stripe products create --live --name "$1" --description "$2" | jq -r .id)
    echo "  created product $1 ($id)" >&2
  fi
  echo "$id"
}

price() { # product lookup_key cents [recurring interval] → id (created when missing)
  local id
  id=$(stripe prices list --live --limit 100 -d "lookup_keys[]=$2" | jq -r '.data[0].id // empty')
  if [ -z "$id" ]; then
    local args=(--live -d "product=$1" -d "currency=usd" -d "unit_amount=$3" -d "lookup_key=$2")
    [ -n "${4:-}" ] && args+=(-d "recurring[interval]=$4")
    id=$(stripe prices create "${args[@]}" | jq -r .id)
    echo "  created price $2 ($id)" >&2
  fi
  echo "$id"
}

echo "→ Stripe (live): products and prices"
PRO=$(product "ScreenForge Pro" "Export plus 1,000 AI agent credits every month.")
CREDITS=$(product "ScreenForge agent credits" "AI agent credits for ScreenForge. They never expire.")
P_PRO=$(price "$PRO" screenforge_pro_monthly 2000 month)
P10=$(price "$CREDITS" screenforge_credits_500 1000)
P25=$(price "$CREDITS" screenforge_credits_1250 2500)
P50=$(price "$CREDITS" screenforge_credits_2500 5000)

echo "→ Stripe (live): webhook events"
WH_URL="https://$REF.supabase.co/functions/v1/stripe-webhook"
WH=$(stripe webhook_endpoints list --live --limit 100 | jq -r --arg u "$WH_URL" '.data[] | select(.url == $u) | .id' | head -1)
[ -n "$WH" ] || { echo "no webhook for $WH_URL: run setup.sh first"; exit 1; }
EVENTS=(checkout.session.completed checkout.session.async_payment_succeeded invoice.paid invoice.payment_failed customer.subscription.deleted charge.refunded charge.dispute.created)
ARGS=(); for e in "${EVENTS[@]}"; do ARGS+=(-d "enabled_events[]=$e"); done
stripe webhook_endpoints update "$WH" --live "${ARGS[@]}" >/dev/null
echo "  $WH listens to ${EVENTS[*]}"

echo "→ Supabase: secrets, schema, functions"
supabase secrets set --project-ref "$REF" STRIPE_PRICE_PRO="$P_PRO" STRIPE_PRICE_TOPUP_10="$P10" STRIPE_PRICE_TOPUP_25="$P25" STRIPE_PRICE_TOPUP_50="$P50" >/dev/null
supabase db push
supabase functions deploy agent credits create-checkout --project-ref "$REF"
supabase functions deploy stripe-webhook --no-verify-jwt --project-ref "$REF"
echo "Done. Pro $P_PRO; packs $P10 / $P25 / $P50."
