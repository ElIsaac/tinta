#!/usr/bin/env bash
# Desinstala el LaunchAgent de spenmouse-receiver en macOS. Sin sudo. Idempotente.
#
# Uso:
#   ./uninstall.sh           detiene y quita el LaunchAgent (conserva logs y llave)
#   ./uninstall.sh --purge   además borra ~/Library/Logs/spenmouse-receiver.log (nunca la llave)

set -euo pipefail

LABEL="dev.spenmouse.receiver"
APP_DIR="${APP_DIR:-$HOME/Applications/Tinta Receiver.app}"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LOG_FILE="$HOME/Library/Logs/spenmouse-receiver.log"
PURGE=0

info() { printf '\033[1;32m::\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }

usage() {
    cat <<'EOF'
Uso: ./uninstall.sh [--purge] [-h|--help]

Detiene el servicio dev.spenmouse.receiver y elimina su LaunchAgent.

Opciones:
  --purge     También elimina ~/Library/Logs/spenmouse-receiver.log (nunca borra ~/.config/spenmouse/key)
  -h, --help  Muestra esta ayuda y sale
EOF
}

while (($# > 0)); do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --purge)
            PURGE=1
            shift
            ;;
        *)
            usage >&2
            warn "Opción desconocida: $1"
            exit 1
            ;;
    esac
done

UID_NUM="$(id -u)"
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true

if [[ -e "$PLIST" ]]; then
    rm -f "$PLIST"
    info "Quitado $PLIST"
else
    info "No había plist instalado en $PLIST"
fi

if [[ -e "$APP_DIR" ]]; then
    rm -rf "$APP_DIR"
    info "Quitado $APP_DIR"
else
    info "No había app instalada en $APP_DIR"
fi

if (( PURGE )); then
    if [[ -e "$LOG_FILE" ]]; then
        rm -f "$LOG_FILE"
        info "Quitado $LOG_FILE"
    else
        info "No había log en $LOG_FILE"
    fi
fi

info "Listo. La llave de emparejamiento (~/.config/spenmouse/key) no se modificó."
