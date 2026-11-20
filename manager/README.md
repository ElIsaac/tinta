# manager: icono de Waybar y popup para TINTA

Un icono en Waybar que muestra el estado del receptor y, al hacer clic, un popup para manejarlo
todo desde la PC: el servicio del receptor, el emparejamiento del teléfono, qué pantalla ve el
teléfono y el despertar por WiFi de `../wake`.

Es un solo script de Python (`spenmouse-manager`, GTK3 + gtk-layer-shell), sin demonio propio.

## Requisitos

- Hyprland (`hyprctl`) y Waybar.
- Python 3.10+, PyGObject, GTK 3 y gtk-layer-shell:
  `sudo dnf install python3-gobject gtk3 gtk-layer-shell` (Fedora) o
  `sudo pacman -S python-gobject gtk3 gtk-layer-shell` (Arch).
- Una Nerd Font para los iconos.
- El receptor ya compilado: `(cd ../receiver && cargo build --release)`.
- Para la sección Despertar: `pkexec` (polkit) con un agente gráfico corriendo, `iw` y `nmcli`.

## Instalar en otra PC

```sh
git clone https://github.com/ElIsaac/tinta ~/tinta
cd ~/tinta/receiver && cargo build --release
cd ../manager && ./install.sh
```

No pide sudo. Lo que hace `install.sh`:

1. Crea el symlink `~/.local/bin/spenmouse-manager` hacia este directorio (no muevas la carpeta
   después; si la mueves, vuelve a correr `./install.sh`).
2. Escribe `~/.config/systemd/user/spenmouse-receiver.service`. La primera vez lo habilita al
   iniciar sesión y lo arranca. Si el receptor corría como unidad transitoria
   (`systemd-run --user --unit=spenmouse-receiver ...`), la detiene y la reemplaza.
3. Agrega el módulo `custom/spenmouse` a la configuración activa de Waybar
   (`~/.config/hypr/waybar/config` o `~/.config/waybar/config*`, siguiendo symlinks), junto a
   `custom/monitor-manager`; si no existe, después de `tray`; si no, al inicio de `modules-right`.
   Agrega también cuatro reglas a `style.css`.
4. Recarga Waybar (`SIGUSR2`) solo si cambió algo.

Antes de modificar un archivo de Waybar guarda una copia al lado con sufijo `.spenmouse-bak`.
Se puede correr las veces que haga falta: si nada cambió no toca nada. **Después de actualizar
los dotfiles (KooL) vuelve a correrlo** para reponer el icono. Con `--no-waybar` omite el paso 3.

### Waybar a mano

```jsonc
"modules-right": ["tray", "custom/spenmouse" /* ... */],

"custom/spenmouse": {
    "exec": "~/.local/bin/spenmouse-manager --status",
    "return-type": "json",
    "interval": 5,
    "signal": 11,
    "on-click": "setsid -f ~/.local/bin/spenmouse-manager"
}
```

Clases CSS del módulo `#custom-spenmouse`: `stopped`, `idle`, `phone`, `streaming`.

## Uso

| Icono | Significa |
|-------|-----------|
| ratón tachado, atenuado | receptor detenido |
| ratón | receptor en marcha, sin teléfono |
| teléfono enlazado | teléfono conectado |
| pantalla transmitiendo | el teléfono está mostrando la pantalla |

Clic en el icono abre el popup; otro clic, `Esc` o la `✕` lo cierran.

- **Receptor.** Estado, Iniciar/Detener, Reiniciar y «Arrancar al iniciar sesión»
  (`systemctl --user enable/disable`).
- **Teléfono.** IP del teléfono conectado. «Emparejar» muestra el QR y la URI; «Ocultar» los
  quita. La URI lleva la llave: el manager no la guarda, no la registra ni la copia al
  portapapeles.
  - **Otras direcciones (VPN).** El teléfono encuentra la PC solo en la red de casa y con el
    hotspot (es automático, no hay nada que configurar). Por una VPN no: marca ahí la IP de la PC
    en la VPN (se proponen las de la máquina que no son la principal ni puentes de
    docker/libvirt) o escríbela a mano. Viajan dentro del QR, así que **hay que emparejar otra
    vez después de cambiarlas**. La app acepta 4 como máximo.
- **Pantalla.** Tercer monitor virtual, espejo del monitor enfocado o espejo de una salida
  concreta, y la escala del monitor virtual. «Aplicar» guarda y reinicia el receptor (si estaba
  detenido, solo guarda).
- **Despertar.** Si está instalado, qué SSIDs de hotspot vigila y la MAC que la laptop usa en la
  red actual (cambia con cada red; es la del paquete WoL). Agrega o quita SSIDs en la lista y
  pulsa «Instalar» / «Aplicar cambios»; «Desinstalar» lo quita todo. Cada acción muestra antes
  el comando exacto y, al confirmar, lo corre con `pkexec` (pide la contraseña en una ventana;
  nunca se guarda). Al aplicar cambios conserva los ajustes de `/etc/spenmouse-wake.conf`
  (`ND_INTERVAL_MS`, `GRACE_S`, ...). Qué hace y cómo probarlo: `../wake/README.md`.

## Dónde guarda cosas

| Archivo | Qué |
|---------|-----|
| `~/.config/spenmouse/receiver.env` | `SPENMOUSE_SCREEN`, `SPENMOUSE_SCALE` (los lee el servicio) y `SPENMOUSE_ALT` (otras direcciones, solo para emparejar). Si no existe: `virtual`, `1.5`, ninguna |
| `~/.config/systemd/user/spenmouse-receiver.service` | el servicio del receptor |
| `$XDG_RUNTIME_DIR/spenmouse/status` | lo escribe el receptor; el manager solo lo lee |

Otras opciones del receptor (`--port`, `--sensitivity`): edita `ExecStart` con
`systemctl --user edit --full spenmouse-receiver` (ojo: `install.sh` reescribe la unidad si
difiere de la suya).

## Comprobar

```sh
spenmouse-manager --selftest    # pruebas internas, no toca nada
spenmouse-manager --status      # el JSON que ve Waybar
systemctl --user status spenmouse-receiver
```

## Desinstalar

```sh
./uninstall.sh           # comando, servicio e icono de Waybar
./uninstall.sh --purge   # además borra receiver.env (nunca la llave de emparejamiento)
```

No toca el despertar por WiFi: quítalo antes desde el popup o con `sudo ../wake/uninstall.sh`.

## Límites conocidos

- El servicio arranca con `graphical-session.target`. Si tu sesión no activa ese target,
  «Arrancar al iniciar sesión» no hará nada: arráncalo desde el autostart del compositor con
  `systemctl --user start spenmouse-receiver`.
- El popup no se cierra al perder el foco (igual que waybar-monitor-manager): el aviso de
  contraseña y los menús desplegables le quitan el foco.
- `pkexec` ejecuta `../wake/install.sh` desde tu carpeta de usuario, igual que `sudo ./install.sh`.
