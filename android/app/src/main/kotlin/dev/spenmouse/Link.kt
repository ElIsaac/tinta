package dev.spenmouse

import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicInteger

private const val HELLO_NS = 1_000_000_000L
private const val WELCOME_TIMEOUT_NS = 3_000_000_000L
private const val HEARTBEAT_NS = 40_000_000L // el contrato pide <= 50 ms; margen para jitter
private const val IDLE_REPEAT_NS = 8_000_000L
private const val IDLE_REPEATS = 3
private const val PAD_WAIT_NS = 20_000_000L
private const val TEXT_RETRY_NS = 150_000_000L
private const val ACTIVE_MASK = IN_RANGE or BTN_LEFT or BTN_RIGHT or BTN_MIDDLE

// Fijos por ejecucion de la app: el receptor conserva (client, last_seq) entre pausas, asi que
// seq no puede reiniciarse al volver a primer plano. Arranca < 2^30 para no llegar al tope u32.
private val CLIENT = SecureRandom().nextInt()
private val SEQ = AtomicInteger(SecureRandom().nextInt() ushr 2)

// Texto sin confirmar: global como SEQ para que sobreviva a las pausas (hay un Link por periodo en
// primer plano y el receptor conserva el ultimo tid de la sesion). Todo bajo synchronized(TEXT).
private val TEXT = StringBuilder()
private var flight: ByteArray? = null
private var TID = 0

/** Al emparejar con otra laptop: lo que no se confirmo no debe teclearse en la nueva. */
fun clearText() = synchronized(TEXT) {
    TEXT.setLength(0)
    flight = null
}

/**
 * Un hilo emisor y uno receptor por periodo en primer plano. No hay cola: [update] pisa el
 * estado pendiente y el emisor siempre manda el mas nuevo.
 */
private const val WOL_PORT = 9
private const val MAC_PREFIX = "mac\t"
private const val CLIP_PREFIX = "clip\t"
private const val ASK_MAC = "\uE025"

/** Paquete magico de Wake-on-LAN: 6 x FF y la MAC 16 veces. null si la MAC no es valida. */
fun magicPacket(mac: String): ByteArray? {
    val b = mac.split(':').mapNotNull { it.toIntOrNull(16) }
    if (b.size != 6 || b.any { it !in 0..255 }) return null
    return ByteArray(6 + 16 * 6) { i -> if (i < 6) 0xFF.toByte() else b[(i - 6) % 6].toByte() }
}

class Link(
    private val pairing: Pairing,
    private val onWorkspaces: (String) -> Unit = {},
    private val macs: () -> Collection<String> = { emptyList() },
    private val onMac: ((String) -> Unit)? = null, // null = no preguntar la MAC
    private val onClip: ((String) -> Unit)? = null, // portapapeles de la laptop
    private val onConnected: (Boolean) -> Unit,
) {
    private val lock = Object()
    private var running = true
    private var dirty = false
    private var wake = false
    private var flags = 0
    private var x = 0
    private var y = 0
    private var pressure = 0
    private var wheel = 0

    @Volatile var padW = 0
    @Volatile var padH = 0

    /** Donde esta la laptop ahora: la IP emparejada, o la que contesto a un HELLO por broadcast. */
    @Volatile var host: InetAddress = InetAddress.getByAddress(pairing.hostBytes)
        private set

    @Volatile private var lastDirect = System.nanoTime() - WELCOME_TIMEOUT_NS

    @Volatile private var session = 0L
    @Volatile private var lastWelcome = 0L
    @Volatile private var helloSeq = 0
    @Volatile private var prevHelloSeq = 0

    fun start() = Thread(::sendLoop, "spen-tx").apply { priority = Thread.MAX_PRIORITY }.start()

    /** El emisor manda el estado inactivo final y cierra el socket por su cuenta. */
    fun stop() = synchronized(lock) {
        running = false
        lock.notify()
    }

    /** Encola texto (U+0008 = BackSpace, U+000A = Enter). Nunca se pierde ni se reordena: ver seccion 8. */
    fun sendText(s: String) {
        synchronized(TEXT) { TEXT.append(s) }
        wakeUp()
    }

    private fun wakeUp() = synchronized(lock) {
        wake = true
        lock.notify()
    }

    /** Paquete STREAM listo para escribir en el TCP, o null si no hay sesion viva. */
    fun streamRequest(w: Int, h: Int): ByteArray? {
        val s = session
        if (s == 0L || System.nanoTime() - lastWelcome >= WELCOME_TIMEOUT_NS) return null
        return ByteArray(STREAM_LEN).also { Codec(hexToBytes(pairing.key)).encodeStream(it, SEQ.incrementAndGet(), s, w, h) }
    }

    fun update(flags: Int, x: Int, y: Int, pressure: Int, wheel: Int) = synchronized(lock) {
        this.flags = flags
        this.x = x
        this.y = y
        this.pressure = pressure
        this.wheel = wheel
        dirty = true
        lock.notify()
    }

    private fun sendLoop() {
        val sock = try {
            DatagramSocket().apply { broadcast = true }
        } catch (e: IOException) {
            return
        }
        Thread({ receiveLoop(sock) }, "spen-rx").start()
        val codec = Codec(hexToBytes(pairing.key))
        val buf = ByteArray(STATE_LEN)
        val pkt = DatagramPacket(buf, 0, host, pairing.port)
        val tbuf = ByteArray(TEXT_HEAD + TEXT_MAX + AEAD_TAG)
        val tpkt = DatagramPacket(tbuf, 0, pkt.address, pkt.port)
        var textDue = false
        var nextText = 0L
        fun send(n: Int) {
            pkt.length = n
            pkt.address = host
            try {
                sock.send(pkt)
            } catch (e: IOException) {
                // sin red o ICMP unreachable: el siguiente paquete lo corrige
            }
        }

        var connected = false
        var nextHello = System.nanoTime()
        var nextState = 0L
        var timer = false
        var repeats = 0
        var f = 0
        var px = 0
        var py = 0
        var pp = 0
        var w = 0
        while (true) {
            var changed = false
            var alive = true
            synchronized(lock) {
                while (running && !dirty && !wake) {
                    var until = if (timer && nextState - nextHello < 0) nextState else nextHello
                    if (textDue && nextText - until < 0) until = nextText
                    val wait = until - System.nanoTime()
                    if (wait <= 0) break
                    lock.wait(wait / 1_000_000, (wait % 1_000_000).toInt())
                }
                alive = running
                changed = dirty
                dirty = false
                wake = false
                f = flags
                px = x
                py = y
                pp = pressure
                w = wheel
            }
            if (!alive) break
            var now = System.nanoTime()
            if (now - nextHello >= 0 && padW == 0) {
                nextHello = now + PAD_WAIT_NS // la vista aun no se mide: no anunciar un area 0x0
            } else if (now - nextHello >= 0) {
                val seq = SEQ.incrementAndGet()
                prevHelloSeq = helloSeq
                helloSeq = seq
                val n = codec.encodeHello(buf, seq, CLIENT, padW, padH)
                send(n)
                // Sin respuesta: la laptop puede estar dormida o en otra red (otra IP). Se la busca por
                // broadcast y se le manda el paquete magico de cada MAC conocida.
                val lost = session == 0L || now - lastWelcome >= WELCOME_TIMEOUT_NS
                // Conectado por una direccion alternativa (VPN) se sigue probando la red local: si contesta, se prefiere.
                if (lost || host.hostAddress in pairing.alt) searchAndWake(sock, buf, n, wake = lost)
                nextHello = now + HELLO_NS
            }
            val s = session
            val c = s != 0L && now - lastWelcome < WELCOME_TIMEOUT_NS
            if (c != connected) {
                connected = c
                onConnected(c)
            }
            if (!c) {
                timer = false
                textDue = false
                continue
            }
            if (changed || (timer && now - nextState >= 0)) {
                send(codec.encodeState(buf, SEQ.incrementAndGet(), s, f, px, py, pp, w))
                now = System.nanoTime()
                if (f and ACTIVE_MASK != 0) {
                    repeats = IDLE_REPEATS - 1
                    nextState = now + HEARTBEAT_NS
                    timer = true
                } else {
                    if (!changed) repeats--
                    else repeats = IDLE_REPEATS - 1
                    nextState = now + IDLE_REPEAT_NS
                    timer = repeats > 0
                }
            }
            // Texto: uno a la vez, despues del STATE para no retrasar el puntero. Lo que se escribe
            // mientras se espera el TACK se junta en el siguiente TEXT.
            var out: ByteArray?
            var tid: Int
            var fresh = false
            synchronized(TEXT) {
                if (flight == null) {
                    flight = takeText(TEXT)
                    if (flight != null) {
                        TID++
                        fresh = true
                    }
                }
                out = flight
                tid = TID
            }
            val o = out
            if (o != null && (fresh || !textDue || now - nextText >= 0)) {
                tpkt.length = codec.encodeText(tbuf, SEQ.incrementAndGet(), s, tid, o)
                try {
                    tpkt.address = host
                    sock.send(tpkt)
                } catch (e: IOException) {
                    // se reintenta en 150 ms
                }
                nextText = System.nanoTime() + TEXT_RETRY_NS
            }
            textDue = o != null
        }
        if (connected) repeat(IDLE_REPEATS) {
            send(codec.encodeState(buf, SEQ.incrementAndGet(), session, 0, px, py, 0, w))
        }
        sock.close()
    }

    private fun searchAndWake(sock: DatagramSocket, hello: ByteArray, n: Int, wake: Boolean) {
        try {
            val nets = java.net.NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
            val targets = nets.filter { it.isUp && !it.isLoopback }.flatMap { it.interfaceAddresses }.mapNotNull { it.broadcast }
            val magics = if (wake) macs().mapNotNull(::magicPacket) else emptyList()
            sock.send(DatagramPacket(hello, n, InetAddress.getByAddress(pairing.hostBytes), pairing.port))
            // direcciones alternativas (VPN): no hay broadcast que las alcance, se prueban una por una
            for (a in pairing.alt) sock.send(DatagramPacket(hello, n, InetAddress.getByName(a), pairing.port))
            for (b in targets) {
                sock.send(DatagramPacket(hello, n, b, pairing.port))
                for (m in magics) sock.send(DatagramPacket(m, m.size, b, WOL_PORT))
            }
        } catch (e: IOException) {
            // sin red: se reintenta en el siguiente HELLO
        }
    }

    private val clipParts = StringBuilder()
    private var clipNext = 0

    private fun receiveLoop(sock: DatagramSocket) {
        val codec = Codec(hexToBytes(pairing.key))
        val buf = ByteArray(WSLIST_MAX)
        val pkt = DatagramPacket(buf, buf.size)
        while (true) {
            try {
                sock.receive(pkt)
            } catch (e: IOException) {
                if (sock.isClosed) return
                continue
            }
            val list = codec.parseWsList(buf, pkt.length, session)
            if (list != null) {
                when {
                    list.startsWith(MAC_PREFIX) -> onMac?.invoke(list.removePrefix(MAC_PREFIX).trim())
                    list.startsWith(CLIP_PREFIX) -> {
                        // clip TAB k TAB n TAB trozo: se arma cuando llegan los n en orden
                        val f = list.split('\t', limit = 4)
                        val k = f.getOrNull(1)?.toIntOrNull()
                        val n = f.getOrNull(2)?.toIntOrNull()
                        if (f.size == 4 && k != null && n != null) {
                            if (k == 1) clipParts.setLength(0)
                            if (k == 1 || k == clipNext) {
                                clipParts.append(f[3])
                                clipNext = k + 1
                                if (k == n) onClip?.invoke(clipParts.toString())
                            } else clipNext = 0 // se perdio un trozo: se descarta hasta el siguiente envio
                        }
                    }
                    else -> onWorkspaces(list)
                }
                continue
            }
            val t = codec.parseTack(buf, pkt.length, session)
            if (t >= 0) {
                val done = synchronized(TEXT) { (flight != null && t.toInt() == TID).also { if (it) flight = null } }
                if (done) wakeUp()
                continue
            }
            val s = codec.parseWelcome(buf, pkt.length)
            if (s == 0L) continue
            // El WELCOME copia el seq del HELLO: solo valen respuestas a los dos ultimos.
            val seq = codec.seqOf(buf)
            if (seq != helloSeq && seq != prevHelloSeq) continue
            val now = System.nanoTime()
            val fresh = s != session || now - lastWelcome >= WELCOME_TIMEOUT_NS
            lastWelcome = now
            session = s
            // Contesto desde aqui: sirve aunque la IP emparejada ya no sea la suya. La red local gana a la
            // VPN: una direccion alternativa solo se usa si la directa lleva rato sin contestar.
            if (pkt.address.hostAddress !in pairing.alt) {
                host = pkt.address
                lastDirect = now
            } else if (now - lastDirect >= WELCOME_TIMEOUT_NS) host = pkt.address
            if (fresh && onMac != null) sendText(ASK_MAC) // la MAC cambia por red: se aprende en cada conexion
            // Sesion nueva o reconexion: despertar al emisor para que mande el estado actual ya.
            if (fresh) synchronized(lock) {
                dirty = true
                lock.notify()
            }
        }
    }
}
