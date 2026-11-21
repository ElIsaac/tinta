# Servicio en segundo plano en macOS (`LaunchAgent`)

Instala `spenmouse-receiver` como `LaunchAgent` de usuario (`dev.spenmouse.receiver`) para que
arranque automáticamente al iniciar sesión, sin necesidad de mantener abierta una ventana de
Terminal.

## Requisitos

1. Compilar el receptor en modo release:

```sh
cd ~/tinta/receiver && cargo build --release
```

2. (Opcional, si aún no has vinculado el teléfono) Emparejar una vez desde la Terminal:

```sh
~/tinta/receiver/target/release/spenmouse-receiver pair
```

## Instalar

```sh
cd ~/tinta/macos
./install.sh
```

No pide `sudo` y es idempotente: puedes volver a ejecutar `./install.sh` con otras opciones para
actualizar el servicio y reiniciarlo.

Opciones disponibles:

| Opción | Variable equivalente | Por defecto | Descripción |
|---|---|---|---|
| `--screen VAL` | `SPENMOUSE_SCREEN` | `main` | Pantalla a capturar: `main` (principal), `virtual` (tercer monitor), `focused` (pantalla bajo el cursor), `window` (ventana única adaptada al tamaño del teléfono) o nombre/ID de pantalla |
| `--scale VAL` | `SPENMOUSE_SCALE` | `1.5` | Factor de escala del monitor virtual |
| `--port VAL` | `SPENMOUSE_PORT` | `47800` | Puerto UDP y TCP del receptor (`1`–`65535`) |

Ejemplos:

```sh
./install.sh --screen virtual --scale 2.0
./install.sh --screen focused --port 47800
./install.sh --screen window
```

Qué instala:

- `~/Applications/Tinta Receiver.app` (bundle `.app` en segundo plano con `LSUIElement` e identificador estable `dev.spenmouse.receiver` para que macOS conserve los permisos TCC)
- `~/Library/LaunchAgents/dev.spenmouse.receiver.plist` (`RunAtLoad`, `KeepAlive`, `ProcessType=Interactive`)
- Carpeta de logs `~/Library/Logs/` con salida estándar y de error en `~/Library/Logs/spenmouse-receiver.log`

## Permisos en macOS (TCC)

Cuando ejecutas `spenmouse-receiver` a mano desde Terminal o iTerm2, macOS suele asociar los
permisos a la aplicación de terminal. Cuando corre como servicio bajo `launchd`, necesitas que el
binario esté empaquetado como `.app` con su `Info.plist` y `CFBundleIdentifier` estable; por eso
`./install.sh` crea `~/Applications/Tinta Receiver.app`.

En **Ajustes del Sistema → Privacidad y seguridad**, agrega y habilita **`Tinta Receiver`**
(`~/Applications/Tinta Receiver.app`) en:

- **Accesibilidad**: necesario para inyectar movimiento, clics, teclado y mover ventanas.
- **Grabación de pantalla**: necesario para capturar video con ScreenCaptureKit.

Si recompilas el binario (`cargo build --release`) y vuelves a ejecutar `./install.sh` con firma
ad-hoc (`CODESIGN_ID=-` por defecto), macOS puede pedir quitar y volver a agregar `Tinta Receiver`
en Privacidad y seguridad (a menos que firmes con un certificado local usando `CODESIGN_ID="..."`).

## Logs y diagnóstico

```sh
# Ver el log en vivo
tail -f ~/Library/Logs/spenmouse-receiver.log

# Ver el estado del servicio en launchd
launchctl print "gui/$(id -u)/dev.spenmouse.receiver"
```

Si `spenmouse-receiver` ya estaba corriendo manualmente en el mismo puerto antes de instalar,
detén esa instancia manual primero para que el servicio pueda enlazar el puerto `47800`.

## Prevención de suspensión y tapa cerrada (clamshell)

- **Suspensión por inactividad (`IOPMAssertion`):** mientras hay una transmisión de video activa,
  el receptor adquiere automáticamente una aserción `kIOPMAssertionTypePreventUserIdleDisplaySleep`
  (`"TINTA video stream active"`). Esto evita que la Mac apague la pantalla o entre en reposo por
  inactividad mientras estás trabajando desde el teléfono. Al detenerse la transmisión o cerrarse
  el receptor, la aserción se libera de inmediato y el equipo vuelve a suspenderse con normalidad.
- **Tapa cerrada (modo clamshell):** `IOPMAssertion` impide el reposo por inactividad, pero **no**
  anula la suspensión inmediata de macOS al cerrar la tapa de una MacBook sin pantalla externa ni
  alimentación conectada. Si quieres usar la MacBook con la tapa cerrada, necesita estar en modo
  clamshell estándar de macOS (alimentación conectada y pantalla externa activa) o usar la
  configuración del sistema que prefieras bajo tu propia responsabilidad.

## Desinstalar

```sh
cd ~/tinta/macos
./uninstall.sh          # detiene y elimina el LaunchAgent (conserva logs y llave)
./uninstall.sh --purge  # además borra ~/Library/Logs/spenmouse-receiver.log
```

Ninguna de las dos opciones borra tu llave de emparejamiento (`~/.config/spenmouse/key`).
