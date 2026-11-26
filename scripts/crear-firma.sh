#!/bin/sh
# Crea, una sola vez, la firma con la que se firman las versiones de Atalaya en este Mac.
#
# macOS recuerda los permisos de una app (acceso a carpetas, Llavero) según su firma.
# Sin firma propia, cada versión nueva cuenta como una app distinta y vuelve a pedirlos.
# Con esta firma, todas las versiones son "la misma app" y heredan los permisos.
#
# El certificado se crea en este Mac y no sale de él: sirve solo aquí. macOS te pedirá
# tu contraseña para marcarlo como confiable para firmar código.
#
# Uso: scripts/crear-firma.sh
set -eu

NOMBRE="Atalaya Local"
LLAVERO="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -v -p codesigning | grep -q "\"$NOMBRE\""; then
  echo "La firma \"$NOMBRE\" ya existe. No hay nada que hacer."
  exit 0
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# Clave de un solo uso para pasar el certificado al Llavero; no protege nada después.
PASE=$(/usr/bin/openssl rand -hex 16)

echo "Creando el certificado…"
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TMP/clave.pem" -out "$TMP/cert.pem" -days 3650 \
  -subj "/CN=$NOMBRE" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null
/usr/bin/openssl pkcs12 -export -inkey "$TMP/clave.pem" -in "$TMP/cert.pem" \
  -out "$TMP/firma.p12" -passout "pass:$PASE" -name "$NOMBRE" 2>/dev/null

echo "Guardándolo en tu Llavero…"
security import "$TMP/firma.p12" -k "$LLAVERO" -P "$PASE" -T /usr/bin/codesign >/dev/null

echo "Marcándolo como confiable para firmar código (macOS te pedirá tu contraseña)…"
security add-trusted-cert -p codeSign -r trustRoot -k "$LLAVERO" "$TMP/cert.pem"

if security find-identity -v -p codesigning | grep -q "\"$NOMBRE\""; then
  echo
  echo "Listo: la firma \"$NOMBRE\" quedó creada."
  echo "Genera el instalador con scripts/build-pkg.sh; saldrá firmado."
  echo "La primera vez, macOS preguntará si codesign puede usar la clave: elige \"Permitir siempre\"."
else
  echo "El certificado se guardó, pero macOS no lo reconoce como válido para firmar." >&2
  echo "Ábrelo en Acceso a Llaveros, busca \"$NOMBRE\" y en Confiar elige \"Confiar siempre\" para Firma de código." >&2
  exit 1
fi
