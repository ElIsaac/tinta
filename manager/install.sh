#!/usr/bin/env bash
# Instala spenmouse-manager: comando, servicio de usuario del receptor e icono de Waybar.
# Sin sudo. Idempotente: volver a correrlo (p. ej. tras actualizar los dotfiles) repone lo que falte.
#
#   ./install.sh               todo
#   ./install.sh --no-waybar   solo el comando y el servicio
#
# Variables: BIN_DIR (~/.local/bin), RECEIVER (../receiver/target/release/spenmouse-receiver)

set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
BIN="$BIN_DIR/spenmouse-manager"
RECEIVER="${RECEIVER:-$(dirname "$SRC_DIR")/receiver/target/release/spenmouse-receiver}"
UNIT=spenmouse-receiver.service
UNIT_FILE="$CONFIG/systemd/user/$UNIT"
DO_WAYBAR=1

case "${1:-}" in
    "") ;;
    --no-waybar) DO_WAYBAR=0 ;;
    *) sed -n '2,9p' "$0"; exit 1 ;;
esac

info() { printf '\033[1;32m::\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }

# ── dependencias ─────────────────────────────────────────────────────────────
missing=()
for c in python3 systemctl hyprctl; do command -v "$c" >/dev/null || missing+=("$c"); done
python3 - <<'PY' 2>/dev/null || missing+=("python3-gobject + gtk3 + gtk-layer-shell")
import gi
gi.require_version("Gtk", "3.0")
gi.require_version("GtkLayerShell", "0.1")
from gi.repository import Gtk, GtkLayerShell
PY
if (( ${#missing[@]} )); then
    warn "Faltan dependencias: ${missing[*]}"
    echo "   Fedora: sudo dnf install python3-gobject gtk3 gtk-layer-shell"
    echo "   Arch:   sudo pacman -S python-gobject gtk3 gtk-layer-shell"
    exit 1
fi
[[ -x "$RECEIVER" ]] || { warn "No existe $RECEIVER: compílalo con (cd ../receiver && cargo build --release)"; exit 1; }
for c in pkexec iw nmcli; do command -v "$c" >/dev/null || warn "Falta $c: la sección Despertar no funcionará completa"; done

# ── comando ──────────────────────────────────────────────────────────────────
# symlink y no copia: el script encuentra ../wake y ../receiver a partir de su ruta real
mkdir -p "$BIN_DIR"
ln -sfn "$SRC_DIR/spenmouse-manager" "$BIN"
info "$BIN -> $SRC_DIR/spenmouse-manager"

# ── servicio de usuario ──────────────────────────────────────────────────────
unit="[Unit]
Description=spen-mouse receiver
After=graphical-session.target
PartOf=graphical-session.target

[Service]
# Opciones que cambia spenmouse-manager (seccion Pantalla)
Environment=SPENMOUSE_SCREEN=virtual SPENMOUSE_SCALE=1.5
EnvironmentFile=-%h/.config/spenmouse/receiver.env
ExecStart=\"$RECEIVER\" --screen \${SPENMOUSE_SCREEN} --scale \${SPENMOUSE_SCALE}
Restart=on-failure

[Install]
WantedBy=graphical-session.target"

first=0; [[ -f "$UNIT_FILE" ]] || first=1
if [[ "$(cat "$UNIT_FILE" 2>/dev/null)" != "$unit" ]]; then
    was_active=0; systemctl --user -q is-active "$UNIT" && was_active=1
    # una unidad transitoria (systemd-run --unit=spenmouse-receiver) tapa a la permanente: hay que pararla
    systemctl --user stop "$UNIT" 2>/dev/null || true
    mkdir -p "$(dirname "$UNIT_FILE")"
    printf '%s\n' "$unit" > "$UNIT_FILE"
    systemctl --user daemon-reload
    if (( first )); then
        systemctl --user enable --now "$UNIT"
        info "Servicio $UNIT creado, habilitado al iniciar sesión y arrancado"
    else
        (( was_active )) && systemctl --user start "$UNIT"
        info "Servicio $UNIT actualizado"
    fi
else
    info "Servicio $UNIT sin cambios"
fi

# ── Waybar ───────────────────────────────────────────────────────────────────
if (( DO_WAYBAR )); then
    WAYBAR_DIR=""
    for d in "$CONFIG/hypr/waybar" "$CONFIG/waybar"; do   # KooL >= 2.3.28 lo tiene bajo hypr/
        [[ -d "$d" ]] && { WAYBAR_DIR="$d"; break; }
    done
    WAYBAR_CONF=""
    for c in config.jsonc config.json config; do
        [[ -n "$WAYBAR_DIR" && -e "$WAYBAR_DIR/$c" ]] && { WAYBAR_CONF="$(readlink -f "$WAYBAR_DIR/$c")"; break; }
    done
    if [[ -z "$WAYBAR_CONF" ]]; then
        warn "No encontré la configuración de Waybar: agrega el módulo a mano (ver README)"
    else
        STYLE=""; [[ -e "$WAYBAR_DIR/style.css" ]] && STYLE="$(readlink -f "$WAYBAR_DIR/style.css")"
        changed="$(python3 - "$WAYBAR_CONF" "$BIN" "$STYLE" <<'PY'
import re, shutil, sys
conf, binary, style = sys.argv[1:]
MODULE = "custom/spenmouse"
BEGIN, END = ">>> spenmouse-manager >>>", "<<< spenmouse-manager <<<"

def save(path, old, new):
    if new == old:
        return
    shutil.copy2(path, path + ".spenmouse-bak")  # el archivo tal como estaba antes de este cambio
    open(path, "w").write(new)
    print(f"{path} (respaldo: {path}.spenmouse-bak)")

src = out = open(conf).read()
block = (f"// {BEGIN}\n"
         f'"{MODULE}": {{\n'
         f'\t"exec": "{binary} --status",\n'
         '\t"return-type": "json",\n'
         '\t"interval": 5,\n'
         '\t"signal": 11,\n'
         f'\t"on-click": "setsid -f {binary}"\n'
         "},\n"
         f"// {END}")
old_block = re.search(rf"// {BEGIN}.*?// {END}", out, re.S)
if old_block:
    out = out[:old_block.start()] + block + out[old_block.end():]
else:
    brace = out.find("{")
    out = out[:brace + 1] + "\n" + block + "\n" + out[brace + 1:]

def in_list(name, text):
    """Primera aparicion de "name" como elemento de una lista (no su definicion ni un comentario)."""
    for m in re.finditer(rf'"{re.escape(name)}"(?!\s*:)', text):
        line = text[text.rfind("\n", 0, m.start()) + 1:m.start()]
        if not line.lstrip().startswith("//"):
            return m
    return None

if not in_list(MODULE, out):
    # junto al icono de monitores; si no esta, tras el tray; si no, al inicio de modules-right
    anchor = in_list("custom/monitor-manager", out) or in_list("tray", out)
    if anchor:
        out = out[:anchor.end()] + f', "{MODULE}"' + out[anchor.end():]
    else:
        m = re.search(r'"modules-right"\s*:\s*\[', out)
        if not m:
            sys.exit(f'!! {conf} no tiene "modules-right": agrega "{MODULE}" a mano')
        out = out[:m.end()] + f'\n\t"{MODULE}",' + out[m.end():]
save(conf, src, out)

if style:
    src = out = open(style).read()
    css = (f"/* {BEGIN} */\n"
           "#custom-spenmouse { padding: 0 6px; }\n"
           "#custom-spenmouse.stopped { opacity: 0.4; }\n"
           "#custom-spenmouse.phone { color: #a7c080; }\n"
           "#custom-spenmouse.streaming { color: #7fbbb3; }\n"
           f"/* {END} */")
    old_css = re.search(rf"/\* {BEGIN} \*/.*?/\* {END} \*/", out, re.S)
    if old_css:
        out = out[:old_css.start()] + css + out[old_css.end():]
    else:
        out = out.rstrip("\n") + "\n\n" + css + "\n"
    save(style, src, out)
PY
)"
        if [[ -n "$changed" ]]; then
            while IFS= read -r line; do info "Modificado $line"; done <<<"$changed"
            # SIGUSR2 = recargar config y estilo conservando los -c/-s con que se lanzo
            pkill -USR2 -x waybar && info "Waybar recargado" || true
        else
            info "Waybar sin cambios"
        fi
    fi
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR no está en tu PATH" ;;
esac
info "Listo. Haz clic en el icono del ratón en Waybar o corre: spenmouse-manager"
