#!/usr/bin/env bash
# Configure Windows code signing for .github/workflows/release.yml with Smartloop's Azure Trusted
# Signing account, the same way smartloop-ai/smartloop-studio signs: GitHub OIDC federation on the
# existing "github-smartloop-signing" app registration — no client secret anywhere.
#
#   scripts/setup-windows-signing.sh      # az login (Documentum tenant, MFA in the browser), add a
#                                         # federated credential for this repo's "release"
#                                         # environment, create that environment, set the secrets
#
# Needs: az (winget install Microsoft.AzureCLI) and gh (logged in). Everything below is discoverable
# from Azure; the defaults are what the account looked like on 2026-10-27.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO=${REPO:-mehfuzh/screenforge}
ENVIRONMENT=${ENVIRONMENT:-release}
TENANT=${AZ_TENANT:-d0707627-32b7-4dfc-9a8d-7e4ba80a7276}          # Documentum (holds the Pay-As-You-Go sub)
SUBSCRIPTION=${SUBSCRIPTION:-eed8c51e-bb6b-4be4-98f3-3c1feef4a77c}
APP_NAME=${APP_NAME:-github-smartloop-signing}                     # app registration the other repos use
TS_ACCOUNT=${TS_ACCOUNT:-smartloop}
TS_PROFILE=${TS_PROFILE:-public-release}
API="api-version=2024-09-30-preview"

command -v gh >/dev/null || { echo "missing: gh (winget install GitHub.cli)"; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "gh is not logged in: run  gh auth login"; exit 1; }
for d in "/c/Program Files/Microsoft SDKs/Azure/CLI2/wbin" "/c/Program Files (x86)/Microsoft SDKs/Azure/CLI2/wbin"; do
  [ -d "$d" ] && export PATH="$PATH:$d"
done
command -v az >/dev/null || { echo "missing: az (winget install Microsoft.AzureCLI, then open a new terminal)"; exit 1; }
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL="*"   # keep /subscriptions/... ids intact under Git Bash

echo "→ Azure login (tenant $TENANT)"
if [ "$(az account show --query tenantId -o tsv 2>/dev/null || true)" != "$TENANT" ]; then
  az login --tenant "$TENANT" --output none
fi
az account set --subscription "$SUBSCRIPTION"
echo "  $(az account show --query "[name,user.name]" -o tsv | tr '\t' ' ')"

echo "→ Trusted Signing account $TS_ACCOUNT"
ACCOUNT_ID=$(az resource list --resource-type Microsoft.CodeSigning/codeSigningAccounts --query "[?name=='$TS_ACCOUNT'].id" -o tsv)
[ -n "$ACCOUNT_ID" ] || { echo "no Trusted Signing account named $TS_ACCOUNT in subscription $SUBSCRIPTION"; exit 1; }
ENDPOINT=$(az rest --method get --url "https://management.azure.com$ACCOUNT_ID?$API" --query properties.accountUri -o tsv | sed 's|/$||')
STATUS=$(az rest --method get --url "https://management.azure.com$ACCOUNT_ID/certificateProfiles/$TS_PROFILE?$API" --query properties.status -o tsv)
[ "$STATUS" = "Active" ] || { echo "certificate profile $TS_PROFILE is '$STATUS', expected Active"; exit 1; }
echo "  $ENDPOINT  profile $TS_PROFILE ($STATUS)"

echo "→ App registration $APP_NAME"
APP_ID=$(az ad app list --display-name "$APP_NAME" --query "[0].appId" -o tsv)
[ -n "$APP_ID" ] || { echo "app registration $APP_NAME not found in tenant $TENANT"; exit 1; }
echo "  client id $APP_ID"
SUBJECT="repo:$REPO:environment:$ENVIRONMENT"
CRED_NAME="github-$(basename "$REPO")-$ENVIRONMENT-env"
if az ad app federated-credential list --id "$APP_ID" --query "[?subject=='$SUBJECT'].name" -o tsv | grep -q .; then
  echo "  federated credential for $SUBJECT already present"
else
  az ad app federated-credential create --id "$APP_ID" --parameters "{\"name\":\"$CRED_NAME\",\"issuer\":\"https://token.actions.githubusercontent.com\",\"subject\":\"$SUBJECT\",\"audiences\":[\"api://AzureADTokenExchange\"],\"description\":\"$REPO release workflow (Windows Trusted Signing)\"}" --output none
  echo "  added federated credential $CRED_NAME ($SUBJECT)"
fi
# The service principal already holds the signer role on the account for the other repos; make
# sure, since a fresh app registration would need it.
SP_ID=$(az ad sp list --filter "appId eq '$APP_ID'" --query "[0].id" -o tsv)
if ! az role assignment list --scope "$ACCOUNT_ID" --assignee "$SP_ID" --query "[?contains(roleDefinitionName,'Certificate Profile Signer')].id" -o tsv | grep -q .; then
  az role assignment create --assignee-object-id "$SP_ID" --assignee-principal-type ServicePrincipal \
    --role "Trusted Signing Certificate Profile Signer" --scope "$ACCOUNT_ID" --output none
  echo "  granted Trusted Signing Certificate Profile Signer"
fi

echo "→ GitHub environment \"$ENVIRONMENT\" and secrets on $REPO"
gh api -X PUT "repos/$REPO/environments/$ENVIRONMENT" >/dev/null
gh secret set AZURE_CLIENT_ID       -R "$REPO" --body "$APP_ID"
gh secret set AZURE_TENANT_ID       -R "$REPO" --body "$TENANT"
gh secret set AZURE_SUBSCRIPTION_ID -R "$REPO" --body "$SUBSCRIPTION"
gh secret set AZURE_TS_ENDPOINT     -R "$REPO" --body "$ENDPOINT"
gh secret set AZURE_TS_ACCOUNT      -R "$REPO" --body "$TS_ACCOUNT"
gh secret set AZURE_TS_PROFILE      -R "$REPO" --body "$TS_PROFILE"
gh secret list -R "$REPO" | grep -E "^AZURE_" || true
echo "done. Trigger the release workflow (Actions → Release → Run workflow) to produce a signed build."
