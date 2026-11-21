# TINTA

**T**INTA **I**s **N**ot a **T**ablet **A**pp.

Convierte un teléfono Android con lápiz (probado en un Galaxy S26 Ultra con S Pen) en mouse,
teclado y **tercer monitor** de una laptop Linux con Hyprland, por la red local, un hotspot o una VPN.

- **Mouse:** lápiz o dedo como touchpad relativo de baja latencia (UDP). El botón del lápiz hace
  clic; sostenido, clic derecho.
- **Teclado:** lo que escribas, dictes o traces a mano en el teléfono se teclea en la laptop.
  Teclas Super, Ctrl, Alt, Shift, Esc, Tab, flechas, Copiar y Pegar.
- **Pantalla:** el teléfono es un monitor más (H.264 por hardware). Se ajusta al girarlo y al abrir
  el teclado.
- **Workspaces:** panel con miniaturas para traer un workspace entero o una sola ventana al teléfono.
- **Modo móvil:** tocar directo, scroll natural, mantener presionado para seleccionar.
- **Portapapeles compartido** entre laptop y teléfono.
- **Despertar la laptop** desde el teléfono por WiFi o al encender el hotspot.
- **Manager de escritorio:** icono en Waybar con un popup para manejar todo y emparejar por QR.

El tráfico va autenticado con una llave compartida; el texto, el portapapeles, los títulos de
ventana y el video van además cifrados (ChaCha20-Poly1305). El protocolo está en
[`CONTRACT.md`](CONTRACT.md).

## Requisitos

**Laptop**

- Linux con **Hyprland 0.57+** (configuración en Lua) y Waybar.
- Captura y codificación: `wf-recorder` y una GPU Intel con VAAPI (`h264_vaapi`).
  Con otra GPU hay que cambiar la orden de captura en `receiver/src/stream.rs`.
- `wtype`, `wl-clipboard`, `iw`, Rust (`cargo`), Python 3 con GTK 3 y gtk-layer-shell.

**Teléfono**

- Android 14 o más reciente. El lápiz es opcional: con el dedo funciona todo menos el hover.

## Instalación en la laptop

```sh
# 1. Dependencias (Fedora; en Arch los paquetes se llaman casi igual)
sudo dnf install cargo wtype wf-recorder wl-clipboard iw python3-gobject gtk3 gtk-layer-shell

# 2. Código y receptor
git clone https://github.com/ElIsaac/tinta ~/tinta
cd ~/tinta/receiver && cargo build --release

# 3. Permiso para crear dispositivos de entrada virtuales (tu usuario debe estar en el grupo `input`)
echo 'KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' \
  | sudo tee /etc/udev/rules.d/70-spenmouse-uinput.rules
sudo udevadm control --reload-rules && sudo udevadm trigger --name-match=uinput

# 4. Puerto de la app (UDP y TCP). Solo si tu firewall no lo tiene abierto ya.
sudo firewall-cmd --permanent --add-port=47800/udp --add-port=47800/tcp && sudo firewall-cmd --reload

# 5. Manager: instala el servicio del receptor (arranca al iniciar sesión) y el icono de Waybar
cd ~/tinta/manager && ./install.sh
```

Si tu usuario no está en `input`: `sudo usermod -aG input $USER` y vuelve a iniciar sesión.

Detalles del manager en [`manager/README.md`](manager/README.md).

## Instalación en macOS

**Requisitos**

- macOS 13 (Ventura) o más reciente (probado en Apple Silicon).
- Command Line Tools de Xcode (`xcode-select --install`) y Rust (`cargo`).

**Compilación**

```sh
git clone https://github.com/ElIsaac/tinta ~/tinta
cd ~/tinta/receiver && cargo build --release
```

**Permisos**

En **Ajustes del Sistema → Privacidad y seguridad**, otorga estos permisos a la terminal desde la que ejecutas el receptor (o a `~/Applications/Tinta Receiver.app` si usas el servicio en segundo plano):

- **Accesibilidad**: necesario para mover el puntero, teclear y mover ventanas entre pantallas.
- **Grabación de pantalla**: necesario para capturar el escritorio, una ventana o el monitor virtual con ScreenCaptureKit.

**Ejecución, servicio y emparejamiento**

```sh
# Emparejar (imprime el QR y la URI; usa --alt IP_DE_LA_VPN si aplica)
~/tinta/receiver/target/release/spenmouse-receiver pair

# Iniciar el receptor a mano (UDP y TCP en el puerto 47800 por defecto)
~/tinta/receiver/target/release/spenmouse-receiver

# O instalar el servicio en segundo plano al iniciar sesión (LaunchAgent)
cd ~/tinta/macos && ./install.sh

# O instalarlo en modo ventana única adaptada al teléfono
cd ~/tinta/macos && ./install.sh --screen window
```

Detalles del servicio en segundo plano, opciones (`--screen` con `main`, `virtual`, `focused`, `window` o nombre/ID, `--scale`, `--port`), logs y
prevención de suspensión en [`macos/README.md`](macos/README.md).

**Límites conocidos en macOS**

- No incluye el manager de escritorio (Waybar) ni el despertar por WiFi (`wake/`).
- La vista de ventanas agrupa y presta ventanas por pantalla (usando la API de Accesibilidad), no por Spaces de Mission Control.
- Las ventanas en pantalla completa nativa de macOS o protegidas contra captura pueden no moverse ni capturarse.

## Instalación en el teléfono

Descarga `tinta.apk` de la página de
[Releases](https://github.com/ElIsaac/tinta/releases) y ábrelo en el teléfono (Android pedirá
permitir instalar desde esa fuente). O por cable:

```sh
adb install tinta.apk
```

El APK publicado está firmado con una llave de depuración: sirve para uso personal, no para
tiendas. Para compilarlo tú (JDK 17 a 21 y el SDK de Android):

```sh
cd ~/tinta/android && ./gradlew assembleDebug
# app/build/outputs/apk/debug/app-debug.apk
```

## Vincular el teléfono

1. En la laptop, clic en el icono del mouse en Waybar y luego en **Emparejar**. Aparece un QR.
2. Escanéalo con la cámara del teléfono y abre el enlace con la app (se llama "S Pen Mouse" en el teléfono).
   Si la cámara no lo abre, copia la URI que sale bajo el QR y pégala en la app con el botón de
   enlace.

El QR lleva todo lo necesario: dirección y puerto de la laptop, la llave, su nombre, su MAC (para
despertarla) y las direcciones extra que marques, por ejemplo la de tu VPN. **Es un secreto:**
quien lo tenga puede controlar la laptop.

Sin el manager, lo mismo desde una terminal:

```sh
~/tinta/receiver/target/release/spenmouse-receiver pair [--alt IP_DE_LA_VPN]
```

## Uso

| Botón | Qué hace |
|---|---|
| Teclado | Abre el teclado del teléfono; lo que escribas se teclea en la laptop |
| Pantalla | El teléfono se vuelve un monitor más |
| Ondas | Modo móvil: tocar directo, scroll con el dedo y más zoom |
| Ventanas | Panel de workspaces: traer uno entero o una sola ventana |
| Enlace | Emparejar pegando la URI |
| Flecha arriba | Ocultar la barra |
| Super, Ctrl, Alt, Shift | Quedan sostenidos hasta la siguiente tecla; también valen con el mouse |

La app encuentra a la laptop sola en la red local y en el hotspot del teléfono. Por VPN usa la
dirección extra del emparejamiento.

## Despertar la laptop

Instalación, pruebas y límites en [`wake/README.md`](wake/README.md). Se instala desde el manager
(sección Despertar) o con `sudo wake/install.sh "NombreDeTuHotspot"`.

## Límites conocidos

- Solo Hyprland: el monitor virtual y el manejo de workspaces usan `hyprctl`.
- La captura de video asume Intel VAAPI.
- El despertar por WiFi depende de la tarjeta y del kernel; hay que probarlo en cada equipo.
- Android solo deja leer el portapapeles con la app al frente: lo copiado en el teléfono se manda
  al volver a la app.
- El movimiento del puntero va autenticado pero sin cifrar; todo lo demás va cifrado.

## Licencia

MIT. Ver [`LICENSE`](LICENSE).
