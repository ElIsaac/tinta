package dev.spenmouse

import java.io.DataInputStream
import java.io.IOException
import java.io.InputStream
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

// Video de la seccion 9 de CONTRACT.md. Sin imports de Android: se prueba en la JVM.

private const val RECORD_MAX = 1 shl 20

/** Lee `np` y luego registros cifrados de la respuesta a STREAM. */
class RecordReader(key: ByteArray, input: InputStream) {
    private val input = DataInputStream(input)
    private val kvid = SecretKeySpec(deriveKey(key, "spenmouse video"), "ChaCha20")
    private val aead = newAead()
    private val nonce = ByteArray(12)
    private var counter = -1L
    private var buf = ByteArray(1 shl 16)

    /** Texto plano del siguiente registro. IOException o GeneralSecurityException = cerrar la conexion. */
    fun next(): ByteArray {
        if (counter < 0) input.readFully(nonce, 0, 4)
        counter++
        for (i in 0..7) nonce[4 + i] = (counter ushr (56 - 8 * i)).toByte()
        val len = input.readInt()
        if (len < AEAD_TAG || len > RECORD_MAX) throw IOException("registro de $len bytes")
        if (buf.size < len) buf = ByteArray(len)
        input.readFully(buf, 0, len)
        aead.init(Cipher.DECRYPT_MODE, kvid, IvParameterSpec(nonce))
        return aead.doFinal(buf, 0, len)
    }
}

/**
 * Parte un flujo H.264 Annex-B que llega en trozos arbitrarios. [onFrame] recibe una unidad de
 * acceso (SPS/PPS/SEI pendientes + un slice, cada NAL con codigo de inicio de 4 bytes) en un
 * buffer que se reutiliza. Un NAL solo se sabe completo al ver el codigo de inicio siguiente.
 */
class AnnexB(private val onFrame: (data: ByteArray, len: Int) -> Unit) {
    private var raw = ByteArray(1 shl 16) // NAL en curso, sin su codigo de inicio
    private var len = 0
    private var scan = 0
    private var inNal = false // false = basura antes del primer codigo de inicio
    private var au = ByteArray(1 shl 16)
    private var auLen = 0

    fun feed(chunk: ByteArray) {
        if (len + chunk.size > raw.size) raw = raw.copyOf(maxOf(raw.size * 2, len + chunk.size))
        System.arraycopy(chunk, 0, raw, len, chunk.size)
        len += chunk.size
        var i = scan
        while (i + 2 < len) {
            if (raw[i + 2].toInt() and 0xFE != 0) i += 3 // ni 0 ni 1: aqui no cabe ningun 00 00 01
            else if (raw[i] != 0.toByte() || raw[i + 1] != 0.toByte() || raw[i + 2] != 1.toByte()) i++
            else {
                var end = i
                while (end > 0 && raw[end - 1] == 0.toByte()) end-- // el cero extra de 00 00 00 01
                if (inNal && end > 0) nal(end)
                inNal = true
                len -= i + 3
                System.arraycopy(raw, i + 3, raw, 0, len)
                i = 0
            }
        }
        scan = maxOf(0, len - 2)
    }

    private fun nal(n: Int) {
        if (auLen + 4 + n > au.size) au = au.copyOf(maxOf(au.size * 2, auLen + 4 + n))
        au[auLen] = 0
        au[auLen + 1] = 0
        au[auLen + 2] = 0
        au[auLen + 3] = 1
        System.arraycopy(raw, 0, au, auLen + 4, n)
        auLen += 4 + n
        if (raw[0].toInt() and 0x1F in 1..5) { // slice: cierra la unidad de acceso
            onFrame(au, auLen)
            auLen = 0
        }
    }
}
