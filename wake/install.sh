#!/usr/bin/bash
# Instala el despertar por WiFi (magic packet + net-detect). Idempotente: se puede volver a correr.
#   sudo ./install.sh [-i IFACE] [SSID_DEL_HOTSPOT ...]
# Sin SSIDs solo queda el magic packet. Valores opcionales por variable de entorno:
#   sudo ND_INTERVAL_MS=20000 ND_DELAY_S=30 GRACE_S=90 WAKE_ON_DISCONNECT=1 PORT=47800 ./install.sh ...
set -euo pipefail
cd "$(dirname "$0")"

iface=
while getopts i: o; do
    case $o in
        i) iface=$OPTARG ;;
        *) echo "uso: sudo $0 [-i IFACE] [SSID ...]" >&2; exit 2 ;;
    esac
done
shift $((OPTIND - 1))

[ "$(id -u)" = 0 ] || { echo "corre con sudo" >&2; exit 1; }
for c in iw ip ss nmcli systemctl systemd-inhibit systemd-escape; do
    command -v "$c" >/dev/null || { echo "falta el comando: $c" >&2; exit 1; }
done

[ -n "$iface" ] || iface=$(iw dev | awk '$1 == "Interface" { print $2; exit }')
[ -n "$iface" ] || { echo "no encontre interfaz WiFi; usa -i IFACE" >&2; exit 1; }
phy=$(cat "/sys/class/net/$iface/phy80211/name") || { echo "$iface no es una interfaz WiFi" >&2; exit 1; }
support=$(iw phy "$phy" info)
grep -q 'wake up on magic packet' <<<"$support" || { echo "$phy no soporta WoWLAN por magic packet" >&2; exit 1; }
if [ $# -gt 0 ] && ! grep -q 'wake up on network detection' <<<"$support"; then
    echo "$phy no soporta net-detect: instala sin SSIDs (solo magic packet)" >&2
    exit 1
fi
for s in "$@"; do
    # shortcut: compara contra el nombre del perfil, que NetworkManager pone igual al SSID; si lo renombraste ignora el aviso
    nmcli -t -f NAME connection show | grep -Fxq -- "$s" ||
        echo "AVISO: no hay perfil de NetworkManager llamado '$s'; conectate una vez a ese hotspot para que se guarde"
done

echo "== /etc/spenmouse-wake.conf"
{
    printf 'SSIDS=('; [ $# -eq 0 ] || printf '%q ' "$@"; printf ')\n'
    echo "ND_INTERVAL_MS=${ND_INTERVAL_MS:-20000}"
    echo "ND_DELAY_S=${ND_DELAY_S:-30}"
    echo "WAKE_ON_DISCONNECT=${WAKE_ON_DISCONNECT:-1}"
    echo "GRACE_S=${GRACE_S:-90}"
    echo "PORT=${PORT:-47800}"
} | tee /etc/spenmouse-wake.conf

echo "== archivos"
install -v -m 0755 spenmouse-wake /usr/local/sbin/spenmouse-wake
install -v -m 0644 spenmouse-wake-arm@.service spenmouse-wake-hold.service /etc/systemd/system/
command -v restorecon >/dev/null && restorecon -v /usr/local/sbin/spenmouse-wake /etc/systemd/system/spenmouse-wake-*.service /etc/spenmouse-wake.conf

echo "== systemd"
# quita instancias de una interfaz anterior antes de habilitar la actual
rm -fv /etc/systemd/system/*.wants/spenmouse-wake-arm@*.service
systemctl daemon-reload
arm_unit="spenmouse-wake-arm@$(systemd-escape "$iface").service"
systemctl enable "$arm_unit" spenmouse-wake-hold.service
systemctl start "$arm_unit" || { journalctl -u "$arm_unit" -n 20 --no-pager; exit 1; }

echo "== estado ($iface, $phy)"
iw phy "$phy" wowlan show
echo "power/wakeup: $(cat "/sys/class/net/$iface/device/power/wakeup") (debe decir enabled)"
echo "MAC actual de $iface (la del magic packet EN ESTA RED): $(cat "/sys/class/net/$iface/address")"
echo "listo. NetworkManager no se modifico. Desinstalar: sudo ./uninstall.sh"
