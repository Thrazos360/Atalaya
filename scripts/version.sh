#!/bin/sh
# Publica una versión nueva de Atalaya: cambia el número, confirma y etiqueta.
# Uso: scripts/version.sh 0.7.0
# Antes, describe la versión en CHANGELOG.md bajo un título "## 0.7.0 — <fecha>".
set -eu
cd "$(dirname "$0")/.."
PATH="$HOME/.cargo/bin:$PATH"

NUEVA=${1:-}
if ! echo "$NUEVA" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "Uso: scripts/version.sh MAYOR.MENOR.PARCHE (por ejemplo 0.7.0)" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/v$NUEVA" >/dev/null; then
  echo "La versión $NUEVA ya existe." >&2
  exit 1
fi
if ! grep -q "^## $NUEVA " CHANGELOG.md; then
  echo "Falta describir la versión en CHANGELOG.md, bajo un título \"## $NUEVA — fecha\"." >&2
  exit 1
fi

# La versión vive en el Cargo.toml de la raíz; package.json solo la refleja.
sed -i '' "s/^version = \".*\"/version = \"$NUEVA\"/" Cargo.toml
sed -i '' "s/^  \"version\": \".*\"/  \"version\": \"$NUEVA\"/" app/package.json
cargo check --workspace --quiet

git add -A
git commit -q -m "Versión $NUEVA"
git tag -a "v$NUEVA" -m "Atalaya $NUEVA"
echo "Versión $NUEVA confirmada y etiquetada como v$NUEVA."
echo "Para generar el instalador: scripts/build-pkg.sh"
