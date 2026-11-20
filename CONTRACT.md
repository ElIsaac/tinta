# spen-mouse: contrato v2

Telefono (S26 Ultra + S Pen) como mouse/tableta de una laptop Linux.
Dos piezas: `android/` (APK, emisor) y `receiver/` (binario Linux, receptor).
Este archivo es la unica fuente de verdad entre ambas. Si algo no esta aqui, no existe.

## 1. Transporte

- UDP, puerto por defecto **47800**. Un datagrama = un paquete.
- Todos los enteros en **big-endian**.
- HELLO/WELCOME/STATE van autenticados sin cifrar (solo coordenadas). El texto tecleado y el
  video de la pantalla si van cifrados (secciones 8 y 9).

## 2. Emparejamiento

Emparejar = que ambos lados tengan la misma llave `K` de 16 bytes.

- El receptor genera `K` con el CSPRNG del sistema la primera vez y la guarda en
  `~/.config/spenmouse/key` (32 hex en minusculas, modo 0600). Borrar el archivo = rotar la llave.
- `spenmouse-receiver pair` imprime un QR en la terminal y la misma URI en texto:

  ```
  spenmouse://pair?host=192.168.0.28&port=47800&key=000102030405060708090a0b0c0d0e0f&name=laptop
  ```

  - `host`: IPv4 de la interfaz de la ruta por defecto (sobrescribible con `--host`).
  - `key`: 32 hex en minusculas. `name`: hostname, solo para mostrar.
  - `mac` (opcional): MAC de la laptop en la red actual, para poder despertarla desde el primer uso.
  - `alt` (opcional): otras IPv4 de la misma laptop separadas por comas (VPN, otra red). El
    telefono manda el HELLO a `host`, a cada `alt` y por broadcast, y usa la que conteste.
- La APK registra el deep link `spenmouse://pair` y guarda host, port, key y name.
  Tres formas de entregarle la URI, todas deben funcionar:
  1. escanear el QR con la camara del telefono,
  2. pegar la URI en un campo de texto dentro de la app,
  3. `adb shell "am start -a android.intent.action.VIEW -d '<uri>'"` (las comillas dobles
     externas son obligatorias: sin ellas el shell del telefono corta la URI en el primer `&`).
- No hay mas handshake de emparejamiento: quien tiene `K` esta emparejado.

## 3. Formato de paquete

Cabecera comun (8 bytes):

| offset | tipo | campo   | valor                         |
|--------|------|---------|-------------------------------|
| 0      | u8x2 | magic   | `0x53 0x4D` ("SM")            |
| 2      | u8   | version | `1`                           |
| 3      | u8   | type    | 1 HELLO, 2 WELCOME, 3 STATE, 4 TEXT, 5 TACK, 6 STREAM, 7 WSLIST |
| 4      | u32  | seq     | ver seccion 5                 |

Todo paquete (salvo TEXT, que usa AEAD) termina con `tag` = primeros **8 bytes** de `HMAC-SHA256(K, todos los bytes anteriores del paquete)`.
Paquete con magic, version, longitud o tag incorrectos: se descarta en silencio.

### HELLO (type 1, telefono -> laptop, 24 bytes)

| offset | tipo | campo  | significado                                         |
|--------|------|--------|-----------------------------------------------------|
| 8      | u32  | client | aleatorio, fijo durante una ejecucion de la app     |
| 12     | u16  | pad_w  | ancho del area tactil en decimas de mm              |
| 14     | u16  | pad_h  | alto del area tactil en decimas de mm               |
| 16     | u8x8 | tag    |                                                     |

### WELCOME (type 2, laptop -> telefono, 24 bytes)

| offset | tipo | campo   | significado                                  |
|--------|------|---------|----------------------------------------------|
| 4      | u32  | seq     | copia del `seq` del HELLO que responde       |
| 8      | u64  | session | id de sesion, aleatorio, nunca 0             |
| 16     | u8x8 | tag     |                                              |

Se envia a la direccion y puerto de origen del HELLO.

### STATE (type 3, telefono -> laptop, 33 bytes)

| offset | tipo | campo    | significado                                              |
|--------|------|----------|----------------------------------------------------------|
| 8      | u64  | session  | el del ultimo WELCOME valido                             |
| 16     | u8   | flags    | ver abajo                                                |
| 17     | u16  | x        | 0..65535, izquierda -> derecha del area tactil           |
| 19     | u16  | y        | 0..65535, arriba -> abajo del area tactil                |
| 21     | u16  | pressure | 0..65535 (0 si no hay contacto o es dedo)                |
| 23     | i16  | wheel    | contador acumulado de pasos de rueda, positivo = arriba  |
| 25     | u8x8 | tag      |                                                          |

`flags`:

| bit | nombre     | significado                                   |
|-----|------------|-----------------------------------------------|
| 0   | IN_RANGE   | lapiz flotando o tocando, o dedo tocando      |
| 1   | CONTACT    | punta o dedo tocando la pantalla              |
| 2   | TOOL_PEN   | 1 = S Pen, 0 = dedo                           |
| 3   | BTN_LEFT   | boton izquierdo presionado                    |
| 4   | BTN_RIGHT  | boton derecho presionado                      |
| 5   | BTN_MIDDLE | boton medio presionado                        |
| 6   | PEN_BUTTON | boton lateral del S Pen presionado            |
| 7   | PEN_ABS    | 1 = absoluto (modo movil): el lapiz o el dedo apuntan donde tocan; 0 = relativo |

STATE es **estado absoluto**, nunca deltas: un paquete perdido se corrige con el siguiente.
`x`, `y` y `pressure` solo tienen sentido con IN_RANGE=1.

## 4. Quien interpreta que

- **El telefono interpreta gestos.** Decide que es un tap, un clic derecho o un scroll y lo
  expresa con BTN_* y `wheel`. El receptor no detecta gestos.
  La APK no envia PEN_BUTTON: pulsacion corta del boton lateral = BTN_LEFT, sostenida 350 ms = BTN_RIGHT.
- **El receptor solo aplica estado** sobre dos dispositivos uinput:
  - *Tableta* (cuando TOOL_PEN=1 y PEN_ABS=1): `ABS_X`, `ABS_Y`, `ABS_PRESSURE`;
    `BTN_TOOL_PEN` = IN_RANGE; `BTN_TOUCH` = CONTACT. Mapeo absoluto sobre la salida que se
    transmite (seccion 9). Con PEN_ABS=0 la tableta queda fuera de rango.
  - *Mouse* (siempre): `BTN_LEFT/RIGHT/MIDDLE` segun los bits; PEN_BUTTON (con TOOL_PEN=1) tambien
    presiona `BTN_RIGHT`; `REL_WHEEL` = diferencia de `wheel` contra el ultimo STATE aceptado
    (aritmetica i16 con wraparound).
  - *Movimiento relativo* (mouse): un STATE "mueve" si es dedo en contacto (TOOL_PEN=0, CONTACT=1)
    o lapiz relativo en rango (TOOL_PEN=1, PEN_ABS=0, IN_RANGE=1, flotando o tocando).
    `REL_X/REL_Y` = diferencia de `x,y` contra el STATE aceptado inmediatamente anterior, solo si
    ese tambien "movia" con el mismo TOOL_PEN; convertida a mm con `pad_w/pad_h` y multiplicada
    por la sensibilidad. Si el anterior no cumplia, este STATE solo fija la referencia.
  - En modo relativo el receptor NO convierte CONTACT del lapiz en clic: el telefono pone
    BTN_LEFT mientras la punta toca.
- El primer STATE de una sesion fija la referencia de `wheel` sin emitir scroll.

## 5. Sesion, orden y vida

- `seq` del telefono: contador u32 que arranca en cualquier valor y sube 1 por cada paquete
  enviado (HELLO y STATE comparten contador).
- El telefono manda HELLO **cada 1 s** mientras la app esta en primer plano. Usa el `session` del
  WELCOME valido mas reciente. Sin WELCOME valido en 3 s = "desconectado" y deja de mandar STATE.
- El receptor mantiene una sola sesion `(client, session, last_seq)`:
  - HELLO valido con el mismo `client` que la sesion actual: responde el mismo `session`.
  - HELLO valido con otro `client` (o sin sesion): crea `session` aleatorio nuevo y olvida `last_seq`.
- El receptor acepta un STATE solo si el tag es valido, `session` coincide con la actual y
  `seq > last_seq` (comparacion simple, sin wraparound). Asi se descartan duplicados, paquetes
  fuera de orden y repeticiones de sesiones viejas.
- **Latido:** mientras IN_RANGE o algun BTN_* este activo, el telefono reenvia el estado actual
  al menos cada 50 ms. Al quedar todo inactivo manda el estado final 3 veces.
- **Seguro contra botones pegados:** si el receptor pasa 500 ms sin aceptar un STATE mientras
  hay algo activo, suelta todos los botones y saca el lapiz de rango.

## 6. Vectores de prueba

`K = 000102030405060708090a0b0c0d0e0f`. Ambas implementaciones deben reproducirlos byte a byte.

| paquete | campos | hex |
|---------|--------|-----|
| HELLO   | seq=1, client=0x01020304, pad_w=1500, pad_h=700 | `534d0101000000010102030405dc02bc8d387c9b70989305` |
| WELCOME | seq=1, session=0x1122334455667788 | `534d0102000000011122334455667788214588e1a8d5a9ae` |
| STATE   | seq=2, session=0x1122334455667788, flags=0x07, x=32768, y=16384, pressure=40000, wheel=-3 | `534d010300000002112233445566778807800040009c40fffd152ce5b8b9bbe8d8` |

## 7. Fuera de alcance en v1

Cifrado, descubrimiento automatico (mDNS/broadcast), varios telefonos a la vez, inclinacion del
lapiz, audio, elegir monitor desde el telefono.

## 8. Texto (teclado)

El telefono manda lo que el usuario escribe (teclado, dictado o escritura a mano con el S Pen
convertida por el teclado de Samsung) y el receptor lo teclea en la ventana enfocada.

- Llaves derivadas: `Ktxt = HMAC-SHA256(K, "spenmouse text")`, `Kvid = HMAC-SHA256(K, "spenmouse video")`
  (32 bytes cada una, los textos en ASCII sin terminador).
- **TEXT (type 4, telefono -> laptop, UDP, 20 + n + 16 bytes, 1 <= n <= 1024):**

  | offset | tipo  | campo   | significado                                   |
  |--------|-------|---------|-----------------------------------------------|
  | 8      | u64   | session | sesion actual                                 |
  | 16     | u32   | tid     | numero de texto, sube 1 por cada texto nuevo  |
  | 20     | bytes | cifrado | ChaCha20-Poly1305: n bytes + 16 de tag        |

  AEAD con llave `Ktxt`, nonce de 12 bytes = `session` (8) seguido del `seq` de la cabecera (4),
  AAD = los primeros 20 bytes del paquete, texto plano = UTF-8.
- En el texto plano: U+0008 = BackSpace, U+000A = Enter, U+0009 = Tab; lo demas se teclea tal cual.
- Teclas especiales (uso privado): U+E001 Izquierda, U+E002 Derecha, U+E003 Arriba, U+E004 Abajo,
  U+E005 Escape. U+E026 copiar y U+E027 pegar: el receptor manda Ctrl+Shift+C/V si la ventana
  enfocada es una terminal y Ctrl+C/V si no. Modificadores en pares abajo/arriba: U+E010/11 Super, U+E012/13 Ctrl,
  U+E014/15 Alt, U+E016/17 Shift (quedan sostenidos entre textos y aplican a teclas y al mouse), U+E020 prestar el workspace activo al monitor del
  telefono (o devolver el prestado). El prestado vuelve solo a su monitor cuando el usuario
  cambia de workspace o cuando el monitor del telefono desaparece. Super queda sostenida entre textos (sirve con teclas
  y con el mouse). El receptor la suelta sola si pasa 3 s sin un HELLO valido.
- Workspaces: U+E021 pide la lista; U+E022 seguido del id en decimal y `;` presta ese workspace
  al telefono (prestar el que ya esta prestado lo devuelve). U+E023 seguido de la direccion
  hexadecimal de una ventana y `;` trae solo esa ventana al workspace que muestra el telefono
  (o la devuelve a su workspace si ya estaba prestada).
- Portapapeles compartido (solo texto). Telefono -> laptop: U+E029, el texto y U+E02A dentro del
  flujo de TEXT (puede ocupar varios paquetes; nunca se teclea). Laptop -> telefono: WSLIST con
  texto `clip TAB k TAB n TAB trozo`, k de 1 a n; el telefono lo arma al recibir los n en orden.
  La laptop lo manda cada vez que cambia su portapapeles y cuando el telefono lo pide con U+E028.
- Despertar: U+E025 pide la MAC con la que hay que despertar a la laptop en esta red; llega en
  un WSLIST cuyo texto es `mac TAB aa:bb:cc:dd:ee:ff`. Desconectado, el telefono manda cada
  segundo el HELLO tambien por broadcast (la IP de la laptop cambia de una red a otra; vale la
  direccion de origen del WELCOME) y un paquete magico (6 x FF + MAC x 16, UDP puerto 9, broadcast)
  por cada MAC que conoce. Ver `wake/README.md` para el lado de la laptop.
- **WSLIST (type 7, laptop -> telefono, UDP, 16 + n + 16 bytes):** cabecera (con `seq` = contador
  propio del receptor), `session` u64 y ChaCha20-Poly1305 con llave
  `Kws = HMAC-SHA256(K, "spenmouse ws")`, nonce = `session` (8) + `seq` (4), AAD = los primeros 16
  bytes. Texto plano UTF-8, una linea por workspace:
  `id TAB monitor TAB mx,my,mw,mh TAB dir,x,y,w,h,clase: titulo TAB ...` (rectangulo logico del monitor y
  de cada ventana, para dibujar una miniatura del acomodo; `dir` es la direccion de la ventana,
  con `*` delante si esta prestada al telefono por separado).
  No se retransmite: si no llega, el telefono vuelve a pedirla.
  Vector (K de prueba, session 0x1122334455667788, seq 9, texto `11<TAB>HDMI-A-1<TAB>kitty: vpn 1<LF>`):
  `534d0107000000091122334455667788b3adba958de12e533cda79eb6a5b849b19c32119d93d01d26c0224c54d0c7673c8ada9d01af0016284`
- El receptor acepta un TEXT si el AEAD verifica, `session` coincide y `seq > last_seq` (misma
  regla y mismo `last_seq` que STATE). Lo aplica solo si `tid` es mayor que el ultimo `tid`
  aplicado en esa sesion (el primero de la sesion siempre se aplica). Siempre responde TACK.
- **TACK (type 5, laptop -> telefono, UDP, 28 bytes):** cabecera con `seq` = copia del `seq` del
  TEXT, `session` u64, `tid` u32, `tag` de 8 bytes (HMAC con `K`, como los demas).
- El telefono manda un texto a la vez: reenvia el mismo `tid` cada 150 ms (con `seq` nuevo y
  cifrado de nuevo) hasta recibir su TACK, y solo entonces manda el siguiente.

## 9. Pantalla (video)

El telefono muestra una salida de la laptop. La salida la elige el receptor (`--screen NOMBRE`
para reflejar un monitor existente, `--screen virtual` para crear un monitor virtual del tamano
del telefono).

- TCP, mismo numero de puerto que UDP. El telefono conecta y manda **STREAM (type 6, 28 bytes):**
  cabecera (con `seq` del contador compartido), `session` u64, `w` u16, `h` u16 (pixeles del
  area de video visible del telefono: orientacion actual y sin lo que tapa el teclado; al
  girarlo o abrir/cerrar el teclado manda un STREAM nuevo), `tag` de 8 bytes (HMAC con `K`).
- El receptor valida tag, sesion y `seq > last_seq`; si falla, cierra sin responder. Un STREAM
  valido reemplaza al anterior: solo hay un video a la vez.
- Respuesta: 4 bytes aleatorios `np`, y luego registros hasta que alguien cierre:
  `len` u32 + `len` bytes de ChaCha20-Poly1305 (llave `Kvid`, nonce = `np` (4) seguido de un
  contador u64 que empieza en 0 y sube 1 por registro, sin AAD). `len` incluye los 16 bytes de
  tag y no pasa de 1 MiB; un registro invalido cierra la conexion.
- La concatenacion de los textos planos es un flujo H.264 Annex-B: empieza con SPS/PPS y un
  keyframe, repite SPS/PPS en cada keyframe, sin B-frames, un slice por cuadro. Los cortes entre
  registros son arbitrarios (no coinciden con NALs).
- El receptor captura solo mientras hay conexion y deja de capturar al cerrarse.
- **Dos modos en la APK.** Mouse: PEN_ABS=0, todo relativo. Movil (solo con video): PEN_ABS=1 y
  `x,y` normalizados sobre el rectangulo del video; el dedo en contacto es un toque de pantalla
  tactil (dispositivo uinput aparte) y el lapiz una tableta absoluta. El receptor ata ambos al
  monitor del telefono. U+E024 seguido de la escala en porcentaje y `;` fija la escala de ese
  monitor desde el siguiente STREAM (el modo movil pide mas zoom).

## 10. Vectores de prueba v2

Con `K = 000102030405060708090a0b0c0d0e0f` y `session = 0x1122334455667788`:

| que | campos | hex |
|-----|--------|-----|
| Ktxt | | `fc70dc61da411f90026cfa60caef55ed81f96a531b26090fb28d900e3b530283` |
| Kvid | | `517c896910f417b1564da3aba8b57147ecaaa06e13f2e04f2c7c97881d29d66c` |
| TEXT | seq=7, tid=3, texto = "Hola ñ" + U+000A + U+0008 | `534d010400000007112233445566778800000003bb59e0163ec12df08c99da7fd50363e129cec0ef926c38c839` |
| TACK | seq=7, tid=3 | `534d01050000000711223344556677880000000321252a0029bd1880` |
| STREAM | seq=8, w=2340, h=1080 | `534d0106000000081122334455667788092404381863742be5c3fe33` |
| registro 0 | np=a1b2c3d4, plano=`000000016742` | `00000016aa9c9b751ebe8610514b7071c1068211f45d08e62765` |
| registro 1 | np=a1b2c3d4, plano=`0000000165` | `000000151d2ab61769c1084f6e26b3b1fa590b79c17b148f66` |
