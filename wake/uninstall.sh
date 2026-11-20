#!/usr/bin/bash
# Deshace install.sh. Idempotente.
set -uo pipefail

[ "$(id -u)" = 0 ] || { echo "corre con sudo" >&2; exit 1; }

echo "== systemd"
systemctl stop spenmouse-wake-hold.service 2>/dev/null
rm -fv /etc/systemd/system/*.wants/spenmouse-wake-arm@*.service \
    /etc/systemd/system/*.wants/spenmouse-wake-hold.service \
    /etc/systemd/system/spenmouse-wake-arm@.service \
    /etc/systemd/system/spenmouse-wake-hold.service
rmdir --ignore-fail-on-non-empty /etc/systemd/system/sys-subsystem-net-devices-*.device.wants 2>/dev/null
systemctl daemon-reload

echo "== archivos"
rm -fv /usr/local/sbin/spenmouse-wake /etc/spenmouse-wake.conf

echo "== WoWLAN"
for p in /sys/class/ieee80211/*; do
    [ -e "$p" ] || continue
    phy=${p##*/}
    echo "+ iw phy $phy wowlan disable"
    iw phy "$phy" wowlan disable
done
echo "listo."
