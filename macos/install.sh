#!/usr/bin/env bash
# Instala el LaunchAgent de spenmouse-receiver en macOS (arranca al iniciar sesión).
# Sin sudo. Idempotente: volver a correrlo actualiza la configuración y reinicia el servicio.
#
# Uso:
#   ./install.sh [--screen main|virtual|focused|window|NOMBRE] [--scale F] [--port N]
#
# Variables opcionales:
#   SPENMOUSE_SCREEN (por defecto: main)
#   SPENMOUSE_SCALE  (por defecto: 1.5)
#   SPENMOUSE_PORT   (por defecto: 47800)
#   RECEIVER         (por defecto: ../receiver/target/release/spenmouse-receiver)
#   APP_DIR          (por defecto: ~/Applications/Tinta Receiver.app)
#   CODESIGN_ID      (por defecto: - [firma ad-hoc])

set -euo pipefail

LABEL="dev.spenmouse.receiver"
SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECEIVER="${RECEIVER:-$(dirname "$SRC_DIR")/receiver/target/release/spenmouse-receiver}"
APP_DIR="${APP_DIR:-$HOME/Applications/Tinta Receiver.app}"
PLIST_DIR="$HOME/Library/LaunchAgents"
PLIST="$PLIST_DIR/$LABEL.plist"
LOG_DIR="$HOME/Library/Logs"
LOG_FILE="$LOG_DIR/spenmouse-receiver.log"
CODESIGN_ID="${CODESIGN_ID:--}"

SCREEN="${SPENMOUSE_SCREEN:-main}"
SCALE="${SPENMOUSE_SCALE:-1.5}"
PORT="${SPENMOUSE_PORT:-47800}"

info() { printf '\033[1;32m::\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die()  { warn "$*"; exit 1; }

usage() {
    cat <<'EOF'
Uso: ./install.sh [opciones]

Instala y arranca el LaunchAgent de spenmouse-receiver para el usuario actual (sin sudo).

Opciones:
  --screen VAL   Pantalla a capturar: main, virtual, focused, window o nombre/ID (por defecto: main)
  --scale VAL    Escala del monitor virtual (por defecto: 1.5)
  --port VAL     Puerto UDP y TCP del receptor, 1..65535 (por defecto: 47800)
  -h, --help     Muestra esta ayuda y sale

Variables de entorno equivalentes:
  SPENMOUSE_SCREEN, SPENMOUSE_SCALE, SPENMOUSE_PORT, RECEIVER, APP_DIR, CODESIGN_ID
EOF
}

xml_escape() {
    local s="$1"
    s="${s//&/&amp;}"
    s="${s//</&lt;}"
    s="${s//>/&gt;}"
    printf '%s' "$s"
}

while (($# > 0)); do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --screen)
            (($# >= 2)) || die "Falta el valor para --screen"
            SCREEN="$2"
            shift 2
            ;;
        --screen=*)
            SCREEN="${1#*=}"
            shift
            ;;
        --scale)
            (($# >= 2)) || die "Falta el valor para --scale"
            SCALE="$2"
            shift 2
            ;;
        --scale=*)
            SCALE="${1#*=}"
            shift
            ;;
        --port)
            (($# >= 2)) || die "Falta el valor para --port"
            PORT="$2"
            shift 2
            ;;
        --port=*)
            PORT="${1#*=}"
            shift
            ;;
        *)
            usage >&2
            die "Opción desconocida: $1"
            ;;
    esac
done

[[ -n "$SCREEN" ]] || die "El valor de --screen no puede estar vacío"

if [[ ! "$SCALE" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]] || ! LC_ALL=C awk -v s="$SCALE" 'BEGIN { exit !(s + 0 > 0) }'; then
    die "Valor inválido para --scale: '$SCALE' (debe ser numérico mayor que 0)"
fi

if [[ ! "$PORT" =~ ^[0-9]+$ ]] || (( 10#$PORT < 1 || 10#$PORT > 65535 )); then
    die "Valor inválido para --port: '$PORT' (debe ser un entero entre 1 y 65535)"
fi
PORT="$((10#$PORT))"

if [[ ! -x "$RECEIVER" ]]; then
    die "No se encontró el binario ejecutable en $RECEIVER. Compílalo primero con: cd ../receiver && cargo build --release"
fi
if [[ "$RECEIVER" != /* ]]; then
    RECEIVER="$(cd "$(dirname "$RECEIVER")" && pwd)/$(basename "$RECEIVER")"
fi

APP_DIR="${APP_DIR%/}"
APP_MACOS="$APP_DIR/Contents/MacOS"
APP_BIN="$APP_MACOS/spenmouse-receiver"
APP_PLIST="$APP_DIR/Contents/Info.plist"
APP_STAMP="$LOG_DIR/.$LABEL.sha256"
UID_NUM="$(id -u)"

mkdir -p "$APP_MACOS" "$PLIST_DIR" "$LOG_DIR"
rm -f "$APP_DIR/Contents/.source-sha256"

launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
for _ in {1..15}; do
    if ! launchctl print "gui/$UID_NUM/$LABEL" >/dev/null 2>&1; then
        break
    fi
    sleep 0.2
done

TMP_INFO="$(mktemp)"
cat > "$TMP_INFO" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>$LABEL</string>
	<key>CFBundleName</key>
	<string>Tinta Receiver</string>
	<key>CFBundleDisplayName</key>
	<string>Tinta Receiver</string>
	<key>CFBundleExecutable</key>
	<string>spenmouse-receiver</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>0.1.0</string>
	<key>CFBundleVersion</key>
	<string>1</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSLocalNetworkUsageDescription</key>
	<string>TINTA receives input and video stream requests from your paired phone on the local network.</string>
</dict>
</plist>
EOF

plutil -lint "$TMP_INFO" >/dev/null

SRC_HASH="$(shasum -a 256 "$RECEIVER" | awk '{print $1}')"
if [[ ! -x "$APP_BIN" ]] \
    || [[ "$(cat "$APP_STAMP" 2>/dev/null || true)" != "$SRC_HASH" ]] \
    || ! cmp -s "$TMP_INFO" "$APP_PLIST" 2>/dev/null \
    || ! codesign -v "$APP_DIR" >/dev/null 2>&1; then
    cp -f "$RECEIVER" "$APP_BIN"
    chmod 0755 "$APP_BIN"
    mv -f "$TMP_INFO" "$APP_PLIST"
    codesign --force --sign "$CODESIGN_ID" --identifier "$LABEL" "$APP_DIR"
    printf '%s\n' "$SRC_HASH" > "$APP_STAMP"
else
    rm -f "$TMP_INFO"
fi

PATH_VAL="${PATH:-/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin}"
ESC_APP_BIN="$(xml_escape "$APP_BIN")"
ESC_PORT="$(xml_escape "$PORT")"
ESC_SCREEN="$(xml_escape "$SCREEN")"
ESC_SCALE="$(xml_escape "$SCALE")"
ESC_LOG="$(xml_escape "$LOG_FILE")"
ESC_HOME="$(xml_escape "$HOME")"
ESC_PATH="$(xml_escape "$PATH_VAL")"

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$LABEL</string>
	<key>ProgramArguments</key>
	<array>
		<string>$ESC_APP_BIN</string>
		<string>--port</string>
		<string>$ESC_PORT</string>
		<string>--screen</string>
		<string>$ESC_SCREEN</string>
		<string>--scale</string>
		<string>$ESC_SCALE</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>$ESC_LOG</string>
	<key>StandardErrorPath</key>
	<string>$ESC_LOG</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>HOME</key>
		<string>$ESC_HOME</string>
		<key>PATH</key>
		<string>$ESC_PATH</string>
	</dict>
</dict>
</plist>
EOF

plutil -lint "$PLIST" >/dev/null

bootstrapped=0
for _ in {1..5}; do
    if launchctl bootstrap "gui/$UID_NUM" "$PLIST" 2>/dev/null; then
        bootstrapped=1
        break
    fi
    sleep 0.3
done
if (( ! bootstrapped )); then
    launchctl bootstrap "gui/$UID_NUM" "$PLIST"
fi
launchctl enable "gui/$UID_NUM/$LABEL"

info "Servicio $LABEL instalado y arrancado"
info "Opciones activas: --port $PORT --screen $SCREEN --scale $SCALE"
info "App bundle: $APP_DIR"
info "Logs: $LOG_FILE"
printf '\nRecuerda otorgar permisos de \033[1mAccesibilidad\033[0m y \033[1mGrabación de pantalla\033[0m en\n'
printf 'Ajustes del Sistema -> Privacidad y seguridad a:\n  %s\n' "$APP_DIR"
printf '(Al correr bajo launchd, no basta con dar permiso solo a la Terminal.)\n'
