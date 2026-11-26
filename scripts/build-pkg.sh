#!/bin/sh
# Arma el instalador de Atalaya para macOS: la app en Aplicaciones, el comando
# `atalaya` en /usr/local/bin y el vigía como servicio en segundo plano.
# Uso: scripts/build-pkg.sh   →  dist/Atalaya-<versión>.pkg
set -eu
cd "$(dirname "$0")/.."
# rustup instala cargo aquí y no siempre está en el PATH de quien llama.
PATH="$HOME/.cargo/bin:$PATH"

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

cargo build --release -p atalaya
(cd app && npm run tauri build)

mkdir -p "$STAGE/root/Applications" "$STAGE/root/usr/local/bin" dist
cp -R target/release/bundle/macos/Atalaya.app "$STAGE/root/Applications/"
cp target/release/atalaya "$STAGE/root/usr/local/bin/atalaya"
# Sin atributos extendidos, para que el paquete no arrastre archivos "._" de sobra.
xattr -cr "$STAGE/root"

# Con la firma propia (scripts/crear-firma.sh), todas las versiones tienen la misma
# identidad y macOS conserva sus permisos de una instalación a la siguiente.
FIRMA="Atalaya Local"
if security find-identity -v -p codesigning | grep -q "\"$FIRMA\""; then
  codesign --force --deep -s "$FIRMA" "$STAGE/root/Applications/Atalaya.app"
  codesign --force -s "$FIRMA" -i cl.thrazos.atalaya.cli "$STAGE/root/usr/local/bin/atalaya"
  echo "Firmado con \"$FIRMA\": los permisos se heredan entre versiones."
else
  echo "Sin firma propia: macOS volverá a pedir permisos en cada versión. Crea la firma con scripts/crear-firma.sh."
fi

# Sin esto, el instalador puede "reubicar" la app sobre otra copia que encuentre en el disco.
pkgbuild --analyze --root "$STAGE/root" "$STAGE/components.plist" >/dev/null
/usr/libexec/PlistBuddy -c "Set :0:BundleIsRelocatable false" "$STAGE/components.plist"

pkgbuild \
  --root "$STAGE/root" \
  --component-plist "$STAGE/components.plist" \
  --scripts scripts/pkg \
  --identifier cl.thrazos.atalaya \
  --version "$VERSION" \
  --install-location / \
  "dist/Atalaya-$VERSION.pkg"

echo "Listo: dist/Atalaya-$VERSION.pkg"
