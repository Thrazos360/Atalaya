#!/bin/sh
# Pasa lo confirmado en este repo al repo público, sin su historial ni datos privados.
#
# Uso: scripts/publicar.sh [--revisar] [--sin-push] [--si] [--ya-integrados] [-m "mensaje"]
#   --revisar        solo revisa y muestra qué cambiaría; no toca el repo público.
#   --sin-push       confirma en el repo público, pero no sube a GitHub.
#   --si             no pregunta antes de subir.
#   --ya-integrados  ya trajiste a este repo los commits que otras personas sumaron al público.
#   -m "mensaje"     mensaje del commit público (por omisión, «Atalaya X.Y.Z»).
#
# Necesita:
#   - el repo público clonado en ATALAYA_PUBLICO (por omisión ../Atalaya-publico), con un
#     correo user.email del tipo …@users.noreply.github.com;
#   - una lista de términos privados en .publicar-prohibido (raíz del repo, fuera de git),
#     uno por línea: nombres, correos, rutas, proyectos. Las líneas con # se ignoran.
#     Sin esa lista no se publica nada.
#
# Qué revisa antes de publicar:
#   1. Que ningún archivo (ni las imágenes) contenga un término de la lista.
#   2. Llaves y tokens con formas conocidas, y archivos sensibles (.env, bases, llaves).
#   3. `atalaya secretos` sobre lo que cambia, si cargo está disponible.
#   4. Que el mensaje del commit tampoco tenga términos privados.
# Los commits públicos llevan hora UTC para no revelar tu zona horaria.
set -eu
cd "$(dirname "$0")/.."
PATH="$HOME/.cargo/bin:$PATH"
PRIVADO=$(pwd)
PUBLICO=${ATALAYA_PUBLICO:-"$PRIVADO/../Atalaya-publico"}
LISTA="$PRIVADO/.publicar-prohibido"

REVISAR=0 PUSH=1 SI=0 INTEGRADOS=0 MENSAJE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --revisar) REVISAR=1 ;;
    --sin-push) PUSH=0 ;;
    --si) SI=1 ;;
    --ya-integrados) INTEGRADOS=1 ;;
    -m) shift; MENSAJE=${1:-} ;;
    -h | --help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Opción desconocida: $1 (usa --help)" >&2; exit 1 ;;
  esac
  shift
done

falla() { echo "✗ $*" >&2; exit 1; }
paso() { echo "→ $*"; }

# ------------------------------------------------------------------ requisitos
[ -d "$PUBLICO/.git" ] || falla "No encuentro el repo público en $PUBLICO (define ATALAYA_PUBLICO)."
PUBLICO=$(cd "$PUBLICO" && pwd)

[ -s "$LISTA" ] || falla "Falta la lista de términos privados en .publicar-prohibido. Sin ella no se publica."
if git ls-files --error-unmatch .publicar-prohibido >/dev/null 2>&1; then
  falla ".publicar-prohibido está en git: sácalo (git rm --cached) antes de publicar."
fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM
PATRONES="$TMP/patrones"
grep -v '^[[:space:]]*#' "$LISTA" | sed 's/[[:space:]]*$//' | grep -v '^$' >"$PATRONES" || true
[ -s "$PATRONES" ] || falla ".publicar-prohibido no tiene términos."

git diff --quiet HEAD -- || falla "Este repo tiene cambios sin confirmar; se publica solo lo confirmado."

CORREO=$(git -C "$PUBLICO" config user.email || true)
case "$CORREO" in
  *@users.noreply.github.com) ;;
  *) falla "El repo público debe firmar con tu correo anónimo de GitHub (git -C $PUBLICO config user.email …@users.noreply.github.com)." ;;
esac
[ -z "$(git -C "$PUBLICO" status --porcelain)" ] || falla "El repo público tiene cambios sin confirmar en $PUBLICO."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || falla "No encuentro la versión en Cargo.toml."

# ------------------------------------------- aportes de otras personas en GitHub
paso "Revisando si hay commits nuevos en el repo público"
git -C "$PUBLICO" fetch -q origin
RAMA=$(git -C "$PUBLICO" rev-parse --abbrev-ref HEAD)
REMOTA="origin/$RAMA"
ULTIMO=$(git config publicar.ultimo || true)
[ -n "$ULTIMO" ] || ULTIMO=$(git -C "$PUBLICO" rev-parse HEAD)
EXTERNOS=$(git -C "$PUBLICO" log --oneline "$ULTIMO..$REMOTA" 2>/dev/null || true)
if [ -n "$EXTERNOS" ]; then
  if [ "$INTEGRADOS" -eq 0 ]; then
    echo "El repo público tiene commits que no salieron de aquí (por ejemplo, pull requests fusionados):" >&2
    echo "$EXTERNOS" | sed 's/^/    /' >&2
    echo "Tráelos primero a este repo, para no borrarlos al publicar:" >&2
    echo "    git fetch $PUBLICO $RAMA && git cherry-pick $ULTIMO..FETCH_HEAD" >&2
    echo "y luego vuelve a correr esto con --ya-integrados." >&2
    exit 1
  fi
  [ "$REVISAR" -eq 1 ] || git -C "$PUBLICO" merge -q --ff-only "$REMOTA"
fi

# ------------------------------------------------- copia de prueba y revisiones
paso "Armando una copia de prueba con lo confirmado (v$VERSION)"
git clone -q --shared "$PUBLICO" "$TMP/prueba"
if [ -n "$EXTERNOS" ]; then git -C "$TMP/prueba" merge -q --ff-only "$REMOTA"; fi
mkdir "$TMP/export"
git archive HEAD | tar -x -C "$TMP/export"
rsync -a --delete --exclude .git "$TMP/export/" "$TMP/prueba/"
git -C "$TMP/prueba" add -A

if git -C "$TMP/prueba" diff --cached --quiet; then
  echo "✓ El repo público ya está al día con este repo."
  exit 0
fi

PROBLEMAS=0
paso "1/4 Términos privados (incluye imágenes y binarios)"
HALLADOS=$(cd "$TMP/export" && grep -r -l -a -i -F -f "$PATRONES" . || true)
if [ -n "$HALLADOS" ]; then
  PROBLEMAS=1
  echo "$HALLADOS" | while read -r f; do
    if grep -I -q . "$TMP/export/$f" 2>/dev/null; then
      (cd "$TMP/export" && grep -n -i -F -f "$PATRONES" "$f" | sed "s|^|    $f:|")
    else
      echo "    $f (binario)"
    fi
  done
fi

paso "2/4 Llaves, tokens y archivos sensibles"
FORMAS='sk-ant-[A-Za-z0-9_-]{20,}|sk-(proj-)?[A-Za-z0-9]{32,}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|glpat-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|xox[abprs]-[A-Za-z0-9-]{10,}|gsk_[A-Za-z0-9]{30,}|hf_[A-Za-z0-9]{30,}|[0-9]{8,10}:AA[A-Za-z0-9_-]{30,}|-----BEGIN [A-Z ]*PRIVATE KEY-----|eyJ[A-Za-z0-9_-]{20,}\.eyJ[A-Za-z0-9_-]{20,}'
TOKENS=$(cd "$TMP/export" && grep -r -n -I -E "$FORMAS" . | grep -v 'atalaya:permitir' || true)
if [ -n "$TOKENS" ]; then PROBLEMAS=1; echo "$TOKENS" | sed 's/^/    /'; fi
SENSIBLES=$(cd "$TMP/export" && find . -type f \( -name '.env*' -o -name '*.db' -o -name '*.sqlite*' -o -name '*.pem' -o -name '*.p12' -o -name '*.key' -o -name 'id_rsa*' -o -name 'id_ed25519*' \) || true)
if [ -n "$SENSIBLES" ]; then PROBLEMAS=1; echo "$SENSIBLES" | sed 's/^/    archivo sensible: /'; fi

paso "3/4 atalaya secretos sobre lo que cambia"
if command -v cargo >/dev/null 2>&1; then
  if ! cargo run -q --offline -p atalaya -- secretos "$TMP/prueba" --desde HEAD; then PROBLEMAS=1; fi
else
  echo "    (cargo no está disponible; se omite)"
fi

if [ -z "$MENSAJE" ]; then
  if git -C "$TMP/prueba" rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null; then
    MENSAJE="Cambios de Atalaya $VERSION"
  else
    MENSAJE="Atalaya $VERSION"
  fi
fi
paso "4/4 Mensaje del commit: «$MENSAJE»"
if printf '%s\n' "$MENSAJE" | grep -q -i -F -f "$PATRONES"; then
  PROBLEMAS=1
  echo "    El mensaje tiene un término privado."
fi

[ "$PROBLEMAS" -eq 0 ] || falla "No se publica: corrige lo de arriba en este repo, confirma y vuelve a intentarlo."
echo "✓ Revisiones limpias."

# Avisos que no detienen la publicación.
grep -q "$VERSION" README.md || echo "! README.md no menciona la versión $VERSION (insignia de versión)."
MENOR=$(echo "$VERSION" | cut -d. -f1,2)
grep -q "$MENOR" web/index.html || echo "! web/index.html no menciona la versión $MENOR."

echo
echo "Lo que cambia en el repo público:"
git -C "$TMP/prueba" diff --cached --stat | tail -25
echo

if [ "$REVISAR" -eq 1 ]; then
  echo "Modo revisión: no se tocó el repo público."
  exit 0
fi

# ---------------------------------------------------------------- publicar
paso "Copiando al repo público"
rsync -a --delete --exclude .git "$TMP/export/" "$PUBLICO/"
git -C "$PUBLICO" add -A
AHORA=$(date -u +%Y-%m-%dT%H:%M:%S+0000)
GIT_AUTHOR_DATE=$AHORA GIT_COMMITTER_DATE=$AHORA git -C "$PUBLICO" commit -q -m "$MENSAJE

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
ETIQUETA=""
if ! git -C "$PUBLICO" rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null; then
  git -C "$PUBLICO" tag "v$VERSION"
  ETIQUETA="v$VERSION"
fi
git config publicar.ultimo "$(git -C "$PUBLICO" rev-parse HEAD)"
echo "✓ Confirmado en el repo público: $(git -C "$PUBLICO" log --oneline -1)"

if [ "$PUSH" -eq 0 ]; then
  echo "Sin subir (--sin-push). Cuando quieras: git -C $PUBLICO push origin $RAMA ${ETIQUETA:+&& git -C $PUBLICO push origin $ETIQUETA}"
  exit 0
fi
if [ "$SI" -eq 0 ]; then
  printf "¿Subir a GitHub ahora? [s/N] "
  read -r RESP
  case "$RESP" in s | S | si | sí | Si | Sí) ;; *) echo "No se subió. Queda confirmado en $PUBLICO."; exit 0 ;; esac
fi
git -C "$PUBLICO" push -q origin "$RAMA"
[ -z "$ETIQUETA" ] || git -C "$PUBLICO" push -q origin "$ETIQUETA"
echo "✓ Publicado. El sitio web se actualiza solo en Vercel."
