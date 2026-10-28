#!/usr/bin/env bash
# Authenticode-sign Windows binaries with Azure Trusted Signing (Smartloop's account).
#   scripts/sign-windows.sh file.exe [more.exe ...]
# Needs, in the environment (the release workflow sets them):
#   SF_SIGNTOOL        path to signtool.exe (Windows 10 SDK, 10.0.22621 or newer)
#   SF_SIGN_DLIB       Azure.CodeSigning.Dlib.dll from the Microsoft.Trusted.Signing.Client NuGet package
#   SF_SIGN_METADATA   json with Endpoint, CodeSigningAccountName, CertificateProfileName
#   an Azure CLI session that can sign with the account (azure/login OIDC in CI, "az login" locally)
# When SF_SIGNTOOL is unset the files are left unsigned and a notice is printed.
set -euo pipefail
if [ -z "${SF_SIGNTOOL:-}" ]; then
  echo "sign-windows: SF_SIGNTOOL not set; leaving unsigned: $*" >&2
  exit 0
fi
: "${SF_SIGN_DLIB:?SF_SIGN_DLIB must point at Azure.CodeSigning.Dlib.dll}"
: "${SF_SIGN_METADATA:?SF_SIGN_METADATA must point at the Trusted Signing metadata json}"
win() { command -v cygpath >/dev/null 2>&1 && cygpath -w "$1" || printf '%s' "$1"; }
for f in "$@"; do
  MSYS2_ARG_CONV_EXCL="*" "$SF_SIGNTOOL" sign /fd SHA256 /td SHA256 \
    /tr http://timestamp.acs.microsoft.com \
    /dlib "$(win "$SF_SIGN_DLIB")" /dmdf "$(win "$SF_SIGN_METADATA")" "$(win "$f")"
done
