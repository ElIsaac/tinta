#!/usr/bin/env bash
# Quita spenmouse-manager: comando, servicio de usuario e icono de Waybar. Sin sudo. Idempotente.
#
#   ./uninstall.sh           conserva ~/.config/spenmouse/receiver.env
#   ./uninstall.sh --purge   tambien borra receiver.env (nunca la llave de emparejamiento)
#
# El despertar por WiFi NO se toca: quitalo antes desde el popup o con sudo ../wake/uninstall.sh

set -euo pipefail

CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
BIN="${BIN_DIR:-$HOME/.local/bin}/spenmouse-manager"
UNIT=spenmouse-receiver.service
UNIT_FILE="$CONFIG/systemd/user/$UNIT"

info() { printf '\033[1;32m::\033[0m %s\n' "$*"; }

rm -f "$BIN" && info "Quitado $BIN"

if [[ -f "$UNIT_FILE" ]]; then
    systemctl --user disable --now "$UNIT" 2>/dev/null || true
    rm -f "$UNIT_FILE"
    systemctl --user daemon-reload
    info "Quitado el servicio $UNIT"
fi

changed=""
for d in "$CONFIG/hypr/waybar" "$CONFIG/waybar"; do
    [[ -d "$d" ]] || continue
    for c in config.jsonc config.json config style.css; do
        [[ -e "$d/$c" ]] || continue
        changed+="$(python3 - "$(readlink -f "$d/$c")" <<'PY'
import re, sys
path = sys.argv[1]
src = open(path).read()
new = re.sub(r"\n?(//|/\*) >>> spenmouse-manager >>>.*?<<< spenmouse-manager <<<( \*/)?\n?", "\n", src, flags=re.S)
new = re.sub(r',\s*"custom/spenmouse"(?!\s*:)|"custom/spenmouse"\s*,?', "", new)
if new != src:
    open(path, "w").write(new)
    print(f":: Limpiado {path}")
PY
)"
    done
    break
done
if [[ -n "$changed" ]]; then
    echo "$changed"
    pkill -USR2 -x waybar || true
fi

if [[ "${1:-}" == "--purge" ]]; then
    rm -f "$CONFIG/spenmouse/receiver.env" && info "Quitado receiver.env"
fi
info "Listo."
