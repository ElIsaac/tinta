# wake: despertar la laptop desde el telefono por WiFi

Dos caminos, los dos quedan armados a la vez:

1. **Net-detect (el camion).** La laptop esta suspendida y sin red. Prendes el hotspot del
   telefono; la tarjeta WiFi ve ese SSID, despierta la laptop y NetworkManager se conecta solo.
2. **Magic packet (la casa).** La laptop se suspende sin soltar el WiFi y despierta cuando el
   telefono manda un paquete WoL en la misma red.

Estado: **escrito y revisado contra el codigo fuente, pero nunca probado con una suspension
real.** Ver "Lo que NO se probo" antes de confiar en el.

## Como funciona

- La tarjeta decide el camino sola, en el instante de suspender: si esta asociada a un AP usa
  magic packet (+ "disconnect"); si no esta asociada usa net-detect. No hay cambio de modo manual.
- `spenmouse-wake-arm@IFACE.service` ejecuta
  `iw phy PHY wowlan enable magic-packet disconnect net-detect interval 20000 delay 30 matches ssid "<hotspot>"`
  en dos momentos: cuando aparece la interfaz (arranque) y justo antes de cada suspension.
  - En el arranque: NetworkManager consulta al kernel al suspender; si ve un disparador que
    conoce (magic packet) **no desconecta** el WiFi. Sin eso lo desconecta y baja la interfaz.
  - Antes de suspender: reaplica por si algo lo borro y levanta la interfaz (con la interfaz
    abajo el kernel no arma nada).
- **Cadena casa -> camion.** Si suspendes en casa, la laptop queda en modo magic packet. Al
  salir de casa pierde el AP, el disparador `disconnect` la despierta, nadie se conecta, se
  vuelve a suspender a los ~90 s y esa segunda vez ya queda en net-detect esperando el hotspot.
- **Tapa cerrada.** Tras despertar con la tapa cerrada, logind vuelve a suspender a los 30 s
  (`HoldoffTimeoutSec`) y sigue reintentando mientras la tapa siga cerrada.
  `spenmouse-wake-hold.service` toma un inhibidor `handle-lid-switch` al despertar y lo
  mantiene `GRACE_S` (90 s) para que el telefono alcance a conectarse, y despues mientras haya
  una conexion TCP de video al puerto del receptor (47800). Al soltarlo, logind suspende solo.
  Con la tapa abierta no hace nada.
- NetworkManager no se toca: ni perfiles ni configuracion. `wake-on-wlan` se queda en `default`.

**Costo en la mochila:** mientras el telefono muestra la pantalla, la laptop esta despierta,
codificando video, cerrada y sin ventilacion: se calienta y gasta bateria. Cada despertar sin
uso (salir de casa, apagar el hotspot, alguien con un SSID igual) cuesta ~90 s despierta.
Suspendida con net-detect la tarjeta escanea cada 20 s, y asociada mantiene el radio vivo:
la bateria en suspension dura menos que sin esto. La alternativa `HandleLidSwitch=ignore` se
descarto: la laptop nunca se suspenderia al cerrarla.

## Requisitos y como revisarlos en cualquier PC

| que | comando | que buscar |
|-----|---------|------------|
| interfaz y phy | `iw dev` | `Interface wlXXX` bajo `phy#N` |
| soporte de la tarjeta | `iw phy phyN info \| grep -A12 'WoWLAN support'` | `wake up on magic packet` (camino 2) y `wake up on network detection, up to N match sets` (camino 1; N = maximo de SSIDs) |
| sintaxis de iw | `iw help wowlan` | que liste `net-detect [interval ... ] [delay ...] ... [matches [ssid <ssid>]+]` |
| NetworkManager con wpa_supplicant | `NetworkManager --print-config \| grep -i backend` | sin `wifi.backend=iwd` |
| nadie mas maneja WoWLAN | `NetworkManager --print-config \| grep wake-on-wlan`; `nmcli -f 802-11-wireless.wake-on-wlan connection show "<perfil>"` | nada / `0x1 (default)`. Si un perfil tiene otro valor, NetworkManager pisa lo de `iw` al conectarse |
| suspension | `cat /sys/power/mem_sleep` | `[s2idle]` o `[deep]` |
| tapa | `systemd-analyze cat-config systemd/logind.conf \| grep -i lid` | por defecto `HandleLidSwitch=suspend` |
| hotspot guardado | `nmcli -f connection.autoconnect,connection.permissions,802-11-wireless.hidden,802-11-wireless-security.psk-flags connection show "<SSID>"` | `autoconnect: yes`, `permissions: --`, `hidden: no`, `psk-flags: 0 (none)` (clave guardada en el sistema, no en el llavero de la sesion) |
| herramientas | `command -v iw ip ss nmcli systemd-inhibit` | todas presentes |

Sin net-detect en la tarjeta solo sirve el camino 2: instala sin SSIDs.

En esta laptop (Fedora 44, kernel 7.2.14, AX211 con iwlmvm, `wlp0s20f3`/`phy0`, s2idle) todo
lo anterior se cumple y el perfil `MiHotspot` ya esta guardado bien.

## Instalar

```sh
cd ~/spen-mouse/wake
# 1. conectate una vez al hotspot del telefono para que quede guardado
# 2. instala (la interfaz se detecta sola; -i IFACE para forzarla; hasta N SSIDs)
sudo ./install.sh "MiHotspot"
```

Solo magic packet: `sudo ./install.sh`. Otros valores:
`sudo ND_INTERVAL_MS=20000 ND_DELAY_S=30 GRACE_S=90 WAKE_ON_DISCONNECT=1 PORT=47800 ./install.sh "SSID"`.
Volver a correrlo reescribe todo con los valores nuevos.

| variable | defecto | significado |
|----------|---------|-------------|
| `ND_INTERVAL_MS` | 20000 | cada cuanto escanea la tarjeta dormida. Menos = despierta antes y gasta mas |
| `ND_DELAY_S` | 30 | espera antes del primer escaneo tras suspender |
| `WAKE_ON_DISCONNECT` | 1 | despertar al perder el AP. Con 0 no hay cadena casa -> camion: solo funciona el hotspot si suspendiste sin red |
| `GRACE_S` | 90 | segundos despierta con la tapa cerrada esperando al telefono |
| `PORT` | 47800 | puerto TCP del receptor que cuenta como "telefono conectado" |

Lo que instala (y nada mas):

- `/etc/spenmouse-wake.conf` (los valores de arriba)
- `/usr/local/sbin/spenmouse-wake`
- `/etc/systemd/system/spenmouse-wake-arm@.service` y `spenmouse-wake-hold.service`, habilitados
- los disparadores WoWLAN del phy (estado del kernel; se pierde al reiniciar y el servicio lo repone)

Revisar despues de instalar (sin suspender):

```sh
sudo iw phy phy0 wowlan show                       # magic packet, disconnect, net detect con tu SSID
cat /sys/class/net/wlp0s20f3/device/power/wakeup   # enabled
```

## Probar, de menos a mas riesgo

Todo con la laptop en el escritorio y enchufada. Abrir la tapa o el boton de encendido siempre
la despiertan. El firmware no despierta en los primeros 10 s de suspension: espera.

**A. Magic packet, tapa abierta (casa).**
1. `cat /sys/class/net/wlp0s20f3/address` -> MAC a usar (ver "Para la app").
2. `systemctl suspend`, espera 20 s.
3. Desde el telefono, en el mismo WiFi, manda el magic packet a esa MAC (mientras la app no lo
   haga, cualquier app "Wake on LAN" sirve). Debe despertar.
4. Repite tras 1 hora suspendida: es donde otros reportan fallos.

**B. Net-detect, tapa abierta.**
1. Hotspot apagado. Deja la laptop sin red **sin bloquear el autoconectado del hotspot**:
   `nmcli connection down "<wifi de casa>"` (y cualquier otra red conocida que se conecte sola).
   Confirma con `iw dev wlp0s20f3 link` -> `Not connected.`
2. `systemctl suspend`, espera 1 minuto (10 s + `ND_DELAY_S` + margen).
3. Prende el hotspot. Debe despertar en menos de ~`ND_INTERVAL_MS` + unos segundos y conectarse
   sola: `nmcli -t -f NAME connection show --active`.
4. Al terminar: `nmcli connection up "<wifi de casa>"`.

**C. Tapa cerrada.** Repite A y B cerrando la tapa en vez de `systemctl suspend`, sin abrir la
app. Debe despertar y volver a suspenderse sola ~90 s despues. Luego repite conectando la app
con video: debe quedarse despierta hasta ~90 s despues de cerrar el video. Evidencia:

```sh
journalctl -b -u systemd-logind -u spenmouse-wake-hold -u 'spenmouse-wake-arm@*' --since -15min
journalctl -b -k --since -15min | grep -i -E 'PM: suspend|iwlwifi'
```

**D. Cadena.** Suspende conectada al hotspot, apaga el hotspot (despierta por `disconnect`),
espera ~2 min a que se resuspenda, prende el hotspot: debe despertar otra vez.

Solo cuando A-D pasen en el escritorio, pruebala en la mochila.

## Desinstalar

```sh
sudo ./uninstall.sh
```

Quita los archivos y servicios y desactiva WoWLAN en todos los phy (`iw phy X wowlan disable`).

## Problemas

- **`power/wakeup` dice `disabled`** tras instalar: el driver no habilito el despertar; revisa
  `sudo iw phy phy0 wowlan show` y `journalctl -u 'spenmouse-wake-arm@*'`.
- **No despierta nunca.** BIOS/UEFI con "Wake on WLAN" apagado; `grep -i -E 'CNVW|WLAN|PXSX' /proc/acpi/wakeup`
  en `disabled`; en `journalctl -k` busca errores de `iwlwifi` al suspender.
- **Magic packet falla a veces.** MAC equivocada (cambia por red, abajo); el router aisla
  clientes o no reenvia broadcast; la tarjeta dormida se salta beacons: manda el paquete
  repetido varios segundos y tambien unicast. En un caso documentado (Framework) se arreglo
  apagando el ahorro de energia WiFi (`wifi.powersave = 2` en NetworkManager).
- **Net-detect no despierta.** Hotspot oculto (no soportado aqui: haria falta `active ssid`),
  hotspot en 6 GHz, SSID escrito distinto, o suspendiste asociada a una red y con
  `WAKE_ON_DISCONNECT=0`. Mira que mando el servicio: `journalctl -u 'spenmouse-wake-arm@*' -n 5`.
- **Despierta y se duerme en ciclo.** El hotspot esta visible pero no logra conectarse (clave
  mala): cada ciclo es despertar, 90 s, dormir. Arregla el perfil o apaga el hotspot.
- **Despierta sola en casa.** El WiFi de casa se cae (`disconnect`). Prueba `WAKE_ON_DISCONNECT=0`
  sabiendo lo que pierdes.
- **Tras despertar no hay WiFi.** Problema conocido de iwlwifi con s2idle en algunas maquinas;
  `sudo modprobe -r iwlmvm && sudo modprobe iwlmvm` (el servicio se rearma solo al reaparecer la interfaz).
- **El hotspot se apaga solo** sin clientes (ajuste de Samsung "apagar cuando no haya dispositivos"):
  prendelo y espera; la laptop tarda hasta `ND_INTERVAL_MS` en verlo.
- **Seguridad.** Cualquiera que emita un SSID con ese nombre o mande un magic packet despierta
  la laptop. No entra (sin la clave no conecta; el receptor autentica), pero gasta bateria.

## Para la app del telefono

**Magic packet:** UDP con 102 bytes de carga: 6 x `FF` seguidos de la MAC de la laptop repetida
16 veces. La tarjeta busca ese patron dentro de la trama; puerto e IP destino no le importan
(usa el 9). Mandalo a la direccion de **broadcast de la subred** de la interfaz WiFi
(`192.168.0.255` en casa; `setBroadcast(true)`, socket atado a la red WiFi para que no salga por
datos) **y** unicast a la ultima IP conocida, repetido (p. ej. cada 200 ms durante 5 s), y
considera despierta a la laptop cuando responda WELCOME.

**Que MAC:** la que la interfaz usa *en esa red*, no la de fabrica. Fedora usa
`wifi.cloned-mac-address=stable-ssid`: una MAC distinta por SSID. En esta laptop: casa
`MiWifiCasa` = `02:11:22:33:44:55`; la del hotspot se ve con `cat /sys/class/net/wlp0s20f3/address`
estando conectada al hotspot. La app debe guardar una MAC por red (o mandar el paquete para
todas las que conozca). Android no deja leer la tabla ARP: hay que capturarla al emparejar.

**En el hotspot cambia:**
- El despertar inicial no usa magic packet: basta prender el hotspot. Pero si la laptop se
  resuspende estando asociada al hotspot (90 s sin video), queda en modo magic packet: la app
  la despierta igual que en casa, con el broadcast de la subred del hotspot y la MAC del hotspot.
- La IP ya no es `192.168.0.28`: la asigna el DHCP del telefono en una subred que Android elige.
  Sin tocar el receptor: manda el HELLO por **broadcast a la subred de la interfaz del hotspot**,
  puerto 47800; el receptor escucha en `0.0.0.0`, solo contesta si el HMAC es valido y manda el
  WELCOME al origen del HELLO; la IP de origen de ese WELCOME es la laptop. Sirve tambien en
  casa si cambia la IP.
- El hotspot no es una `Network` de `ConnectivityManager`: hay que encontrar su interfaz con
  `NetworkInterface` y atar el socket a su direccion local.

## Lo que NO se probo

Nada de esto corrio con una suspension real; `install.sh` no se ha ejecutado.

- Que la AX211 despierte de s2idle por net-detect o por magic packet en este kernel.
- Que NetworkManager de verdad deje el WiFi asociado al suspender con los disparadores puestos
  por `iw` (sale de leer su codigo, version 1.56.0).
- Que logind resuspenda solo al soltar el inhibidor (sale de leer su codigo, v259).
- La cadena `disconnect` -> resuspension -> net-detect.
- Que el firmware compare el magic packet contra la MAC clonada y no la de fabrica.
- Que NetworkManager se conecte rapido al hotspot tras despertar.
- El descubrimiento por broadcast de HELLO y el magic packet enviado desde el hotspot de Android.
- Consumo real de bateria suspendida, y temperatura en la mochila.
- Otras tarjetas/drivers, backend iwd, suspension `deep`.

Lo que si se verifico sin root: sintaxis (`bash -n`, shellcheck), el comando `iw` que arma el
script (`DRY_RUN=1`, SSID con espacios incluido), las unidades con `systemd-analyze verify`, y
que `ss` detecta la conexion TCP de video del telefono.
