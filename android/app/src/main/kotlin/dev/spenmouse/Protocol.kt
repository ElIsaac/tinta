package dev.spenmouse

import java.net.URLDecoder
import java.security.GeneralSecurityException
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

// Protocolo de CONTRACT.md v2. Sin imports de Android: se prueba en la JVM.

const val HELLO_LEN = 24
const val WELCOME_LEN = 24
const val STATE_LEN = 33
const val TACK_LEN = 28
const val STREAM_LEN = 28
const val TEXT_HEAD = 20
const val TEXT_MAX = 1024
const val AEAD_TAG = 16
const val DEFAULT_PORT = 47800

const val IN_RANGE = 1
const val CONTACT = 2
const val TOOL_PEN = 4
const val BTN_LEFT = 8
const val BTN_RIGHT = 16
const val BTN_MIDDLE = 32
const val PEN_BUTTON = 64
const val PEN_ABS = 128

private const val TAG_LEN = 8
private const val TYPE_HELLO = 1
private const val TYPE_WELCOME = 2
private const val TYPE_STATE = 3
private const val TYPE_TEXT = 4
private const val TYPE_TACK = 5
private const val TYPE_STREAM = 6
private const val TYPE_WSLIST = 7
private const val WSLIST_HEAD = 16
const val WSLIST_MAX = 1500

fun hexToBytes(hex: String) = ByteArray(hex.length / 2) { hex.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

/** Ktxt / Kvid del contrato: HMAC-SHA256(K, etiqueta ASCII). */
fun deriveKey(k: ByteArray, label: String): ByteArray =
    Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(k, "HmacSHA256"))
        doFinal(label.toByteArray(Charsets.US_ASCII))
    }

/** El JDK lo llama "ChaCha20-Poly1305"; Conscrypt (Android) "ChaCha20/Poly1305/NoPadding". */
fun newAead(): Cipher = try {
    Cipher.getInstance("ChaCha20-Poly1305")
} catch (e: GeneralSecurityException) {
    Cipher.getInstance("ChaCha20/Poly1305/NoPadding")
}

/** No es thread-safe (Mac y digest se reutilizan): una instancia por hilo. */
class Codec(key: ByteArray) {
    private val mac = Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(key, "HmacSHA256")) }
    private val digest = ByteArray(32)
    private val ktxt by lazy { SecretKeySpec(deriveKey(key, "spenmouse text"), "ChaCha20") }
    private val aead by lazy { newAead() }
    private val kws by lazy { SecretKeySpec(deriveKey(key, "spenmouse ws"), "ChaCha20") }

    fun encodeHello(b: ByteArray, seq: Int, client: Int, padW: Int, padH: Int): Int {
        header(b, TYPE_HELLO, seq)
        put32(b, 8, client)
        put16(b, 12, padW)
        put16(b, 14, padH)
        sign(b, HELLO_LEN - TAG_LEN)
        return HELLO_LEN
    }

    fun encodeState(b: ByteArray, seq: Int, session: Long, flags: Int, x: Int, y: Int, pressure: Int, wheel: Int): Int {
        header(b, TYPE_STATE, seq)
        put64(b, 8, session)
        b[16] = flags.toByte()
        put16(b, 17, x)
        put16(b, 19, y)
        put16(b, 21, pressure)
        put16(b, 23, wheel)
        sign(b, STATE_LEN - TAG_LEN)
        return STATE_LEN
    }

    /** [b] debe medir al menos TEXT_HEAD + plain.size + AEAD_TAG. Cada llamada exige un seq nuevo (nonce). */
    fun encodeText(b: ByteArray, seq: Int, session: Long, tid: Int, plain: ByteArray): Int {
        header(b, TYPE_TEXT, seq)
        put64(b, 8, session)
        put32(b, 16, tid)
        val nonce = ByteArray(12)
        System.arraycopy(b, 8, nonce, 0, 8)
        System.arraycopy(b, 4, nonce, 8, 4)
        aead.init(Cipher.ENCRYPT_MODE, ktxt, IvParameterSpec(nonce))
        aead.updateAAD(b, 0, TEXT_HEAD)
        return TEXT_HEAD + aead.doFinal(plain, 0, plain.size, b, TEXT_HEAD)
    }

    fun encodeStream(b: ByteArray, seq: Int, session: Long, w: Int, h: Int): Int {
        header(b, TYPE_STREAM, seq)
        put64(b, 8, session)
        put16(b, 16, w)
        put16(b, 18, h)
        sign(b, STREAM_LEN - TAG_LEN)
        return STREAM_LEN
    }

    /** Devuelve el session del WELCOME, o 0 si el paquete no es valido (session nunca es 0). */
    fun parseWelcome(b: ByteArray, len: Int): Long = if (valid(b, len, WELCOME_LEN, TYPE_WELCOME)) get64(b, 8) else 0

    /** Devuelve el tid (u32) del TACK, o -1 si el paquete no es valido o es de otra sesion. */
    fun parseTack(b: ByteArray, len: Int, session: Long): Long =
        if (valid(b, len, TACK_LEN, TYPE_TACK) && get64(b, 8) == session) get32(b, 16).toLong() and 0xFFFFFFFFL else -1

    private fun valid(b: ByteArray, len: Int, want: Int, type: Int): Boolean {
        if (len != want || b[0] != 0x53.toByte() || b[1] != 0x4D.toByte() ||
            b[2] != 1.toByte() || b[3] != type.toByte()
        ) return false
        mac(b, want - TAG_LEN)
        var diff = 0
        for (i in 0 until TAG_LEN) diff = diff or (digest[i].toInt() xor b[want - TAG_LEN + i].toInt())
        return diff == 0
    }

    /** Texto de un WSLIST de esta sesion, o null si el paquete no lo es o no verifica. */
    fun parseWsList(b: ByteArray, len: Int, session: Long): String? {
        if (len < WSLIST_HEAD + AEAD_TAG || b[0] != 0x53.toByte() || b[1] != 0x4D.toByte() ||
            b[2] != 1.toByte() || b[3] != TYPE_WSLIST.toByte() || get64(b, 8) != session
        ) return null
        val nonce = ByteArray(12)
        System.arraycopy(b, 8, nonce, 0, 8)
        System.arraycopy(b, 4, nonce, 8, 4)
        return try {
            aead.init(Cipher.DECRYPT_MODE, kws, IvParameterSpec(nonce))
            aead.updateAAD(b, 0, WSLIST_HEAD)
            String(aead.doFinal(b, WSLIST_HEAD, len - WSLIST_HEAD), Charsets.UTF_8)
        } catch (e: GeneralSecurityException) {
            null
        }
    }

    fun seqOf(b: ByteArray) = get32(b, 4)

    private fun header(b: ByteArray, type: Int, seq: Int) {
        b[0] = 0x53
        b[1] = 0x4D
        b[2] = 1
        b[3] = type.toByte()
        put32(b, 4, seq)
    }

    // shortcut: Mac.doFinal(buf, off) asigna 32 bytes por dentro en el JDK/Conscrypt; evitarlo exige
    // un SHA-256 propio. Solo vale la pena si un perfil muestra pausas de GC.
    private fun mac(b: ByteArray, n: Int) {
        mac.update(b, 0, n)
        mac.doFinal(digest, 0)
    }

    private fun sign(b: ByteArray, n: Int) {
        mac(b, n)
        System.arraycopy(digest, 0, b, n, TAG_LEN)
    }

    private fun put16(b: ByteArray, o: Int, v: Int) {
        b[o] = (v ushr 8).toByte()
        b[o + 1] = v.toByte()
    }

    private fun put32(b: ByteArray, o: Int, v: Int) {
        put16(b, o, v ushr 16)
        put16(b, o + 2, v)
    }

    private fun put64(b: ByteArray, o: Int, v: Long) {
        put32(b, o, (v ushr 32).toInt())
        put32(b, o + 4, v.toInt())
    }

    private fun get64(b: ByteArray, o: Int) = (get32(b, o).toLong() shl 32) or (get32(b, o + 4).toLong() and 0xFFFFFFFFL)

    private fun get32(b: ByteArray, o: Int) =
        (b[o].toInt() and 0xFF shl 24) or (b[o + 1].toInt() and 0xFF shl 16) or
            (b[o + 2].toInt() and 0xFF shl 8) or (b[o + 3].toInt() and 0xFF)
}

/** [alt] = otras direcciones de la misma laptop (VPN, otra red) que se prueban cuando [host] no contesta. */
class Pairing(
    val host: String,
    val port: Int,
    val key: String,
    val name: String,
    val alt: List<String> = emptyList(),
    val macs: List<String> = emptyList(), // para despertar la laptop antes de la primera conexion
) {
    val hostBytes get() = ByteArray(4) { host.split('.')[it].toInt().toByte() }

    companion object {
        private const val PREFIX = "spenmouse://pair?"
        private const val MAX_ALT = 4
        private val IPV4 = Regex("""(25[0-5]|2[0-4]\d|1?\d?\d)(\.(25[0-5]|2[0-4]\d|1?\d?\d)){3}""")
        private val KEY = Regex("[0-9a-fA-F]{32}")
        private val MAC = Regex("[0-9a-f]{2}(:[0-9a-f]{2}){5}")

        /** Lanza IllegalArgumentException con un mensaje para mostrar al usuario. */
        fun parse(text: String): Pairing {
            val uri = text.trim()
            require(uri.startsWith(PREFIX)) { "La URI debe empezar con $PREFIX" }
            val q = HashMap<String, String>()
            for (part in uri.substring(PREFIX.length).split('&')) {
                val kv = part.split('=', limit = 2)
                if (kv.size == 2) q[kv[0]] = kv[1]
            }
            val host = q["host"].orEmpty()
            require(IPV4.matches(host)) { "host no es una IPv4 valida: \"$host\"" }
            val port = if ("port" in q) q["port"]!!.toIntOrNull() ?: 0 else DEFAULT_PORT
            require(port in 1..65535) { "port fuera de rango (1-65535): \"${q["port"]}\"" }
            val key = q["key"].orEmpty()
            require(KEY.matches(key)) { "key debe tener exactamente 32 caracteres hexadecimales" }
            val name = runCatching { URLDecoder.decode(q["name"].orEmpty(), "UTF-8") }.getOrDefault("")
            val alt = q["alt"].orEmpty().split(',').filter { IPV4.matches(it) }.take(MAX_ALT)
            val macs = q["mac"].orEmpty().lowercase().split(',').filter { MAC.matches(it) }.take(MAX_ALT)
            return Pairing(host, port, key.lowercase(), name.take(64).ifBlank { host }, alt, macs)
        }
    }
}
