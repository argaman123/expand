#!/usr/bin/env bash
# Signs Expand.app with a private identity made on this Mac, so macOS keeps the app's
# permissions (Screen Recording, Accessibility) across updates. Without it,
# every build gets a new ad-hoc signature and macOS treats it as a different app.
#
# The identity lives in its own keychain under ~/.expand-signing; the login keychain, the
# keychain search list and the system's trust settings are left alone.
set -euo pipefail

APP="${1:-/Applications/Expand.app}"
DIR="$HOME/.expand-signing"
KC="$DIR/signing.keychain-db"
NAME="Expand Local Signing"

mkdir -p "$DIR"
chmod 700 "$DIR"

if [ ! -f "$KC" ]; then
  /usr/bin/openssl rand -hex 24 > "$DIR/password"
  chmod 600 "$DIR/password"
  PASS="$(cat "$DIR/password")"
  cat > "$DIR/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
  /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$DIR/cert.cnf" \
    -keyout "$DIR/key.pem" -out "$DIR/cert.pem" 2>/dev/null
  /usr/bin/openssl pkcs12 -export -inkey "$DIR/key.pem" -in "$DIR/cert.pem" \
    -out "$DIR/identity.p12" -passout pass:"$PASS"
  security create-keychain -p "$PASS" "$KC"
  security set-keychain-settings "$KC"
  security unlock-keychain -p "$PASS" "$KC"
  security import "$DIR/identity.p12" -k "$KC" -P "$PASS" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$PASS" "$KC" >/dev/null
  rm -f "$DIR/key.pem" "$DIR/identity.p12"
fi

security unlock-keychain -p "$(cat "$DIR/password")" "$KC"
HASH="$(/usr/bin/openssl x509 -in "$DIR/cert.pem" -noout -fingerprint -sha1 | sed 's/.*=//; s/://g')"
codesign --force --deep --keychain "$KC" --sign "$HASH" "$APP"
codesign --display --requirements - "$APP" 2>&1 | grep designated
