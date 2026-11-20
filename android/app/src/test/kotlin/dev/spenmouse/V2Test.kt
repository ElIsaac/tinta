package dev.spenmouse

import java.io.ByteArrayInputStream
import java.io.EOFException
import java.security.GeneralSecurityException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

/** Vectores de la seccion 10 de CONTRACT.md y las piezas puras de texto y video. */
class V2Test {
    private val key = hexToBytes("000102030405060708090a0b0c0d0e0f")
    private val session = 0x1122334455667788L
    private val codec = Codec(key)
    private val tack = "534d01050000000711223344556677880000000321252a0029bd1880"
    private val rec0 = "00000016aa9c9b751ebe8610514b7071c1068211f45d08e62765"
    private val rec1 = "000000151d2ab61769c1084f6e26b3b1fa590b79c17b148f66"

    private fun hex(b: ByteArray, n: Int = b.size) = b.take(n).joinToString("") { "%02x".format(it) }

    @Test
    fun derivedKeys() {
        assertEquals("fc70dc61da411f90026cfa60caef55ed81f96a531b26090fb28d900e3b530283", hex(deriveKey(key, "spenmouse text")))
        assertEquals("517c896910f417b1564da3aba8b57147ecaaa06e13f2e04f2c7c97881d29d66c", hex(deriveKey(key, "spenmouse video")))
    }

    @Test
    fun textVector() {
        val b = ByteArray(64)
        val n = codec.encodeText(b, 7, session, 3, "Hola ñ\n\b".toByteArray())
        assertEquals("534d010400000007112233445566778800000003bb59e0163ec12df08c99da7fd50363e129cec0ef926c38c839", hex(b, n))
    }

    @Test
    fun tackVector() {
        val b = hexToBytes(tack)
        assertEquals(3L, codec.parseTack(b, b.size, session))
        assertEquals(7, codec.seqOf(b))
        assertEquals("otra sesion", -1L, codec.parseTack(b, b.size, session + 1))
        assertEquals(-1L, codec.parseTack(b, b.size - 1, session))
        for (i in 0 until TACK_LEN) {
            val t = hexToBytes(tack)
            t[i] = (t[i].toInt() xor 1).toByte()
            assertEquals("byte $i alterado", -1L, codec.parseTack(t, t.size, session))
        }
    }

    @Test
    fun streamVector() {
        val b = ByteArray(STREAM_LEN)
        assertEquals(STREAM_LEN, codec.encodeStream(b, 8, session, 2340, 1080))
        assertEquals("534d0106000000081122334455667788092404381863742be5c3fe33", hex(b))
    }

    @Test
    fun recordVectors() {
        val r = RecordReader(key, ByteArrayInputStream(hexToBytes("a1b2c3d4$rec0$rec1")))
        assertEquals("000000016742", hex(r.next()))
        assertEquals("0000000165", hex(r.next()))
        assertThrows(EOFException::class.java) { r.next() }
    }

    @Test
    fun recordsRejected() {
        for (i in 4 until rec0.length / 2) { // cualquier bit del cifrado o del tag
            val b = hexToBytes("a1b2c3d4$rec0")
            b[4 + i] = (b[4 + i].toInt() xor 1).toByte()
            assertThrows("byte $i", GeneralSecurityException::class.java) { RecordReader(key, ByteArrayInputStream(b)).next() }
        }
        // Registro repetido: el contador ya no coincide.
        val r = RecordReader(key, ByteArrayInputStream(hexToBytes("a1b2c3d4$rec0$rec0")))
        r.next()
        assertThrows(GeneralSecurityException::class.java) { r.next() }
        for (len in listOf("0000000f", "00100001")) assertThrows(java.io.IOException::class.java) {
            RecordReader(key, ByteArrayInputStream(hexToBytes("a1b2c3d4$len" + "00".repeat(32)))).next()
        }
    }

    @Test
    fun diff() {
        assertEquals("a", textDiff("hol", "hola")) // agregar
        assertEquals("\b", textDiff("hola", "hol")) // borrar
        assertEquals("\b\b\b\b", textDiff("hola", ""))
        assertEquals("\b\bhe casa", textDiff("la teh", "la the casa")) // autocorreccion al final
        assertEquals("\b".repeat(7) + "H mundo", textDiff("h mundo", "H mundo")) // edicion a media linea: se reescribe la cola
        assertEquals("", textDiff("igual", "igual"))
        assertEquals("ñ", textDiff("", "ñ"))
        // Un emoji es un solo BackSpace y nunca se parte el par sustituto.
        assertEquals("\b", textDiff("a😀", "a"))
        assertEquals("\b😁", textDiff("a😀", "a😁"))
        assertEquals("", textDiff("a😀", "a😀"))
    }

    @Test
    fun takeTextSplitsOnCharacters() {
        val sb = StringBuilder("añ😀€b") // 1 + 2 + 4 + 3 + 1 bytes
        assertArrayEquals("añ".toByteArray(), takeText(sb, 6))
        assertArrayEquals("😀".toByteArray(), takeText(sb, 6))
        assertArrayEquals("€b".toByteArray(), takeText(sb, 6))
        assertNull(takeText(sb, 6))
        val big = StringBuilder("ñ".repeat(600))
        assertEquals(TEXT_MAX, takeText(big)!!.size)
        assertEquals(88, big.length)
    }

    private val stream = hexToBytes(
        "ffff" + // basura antes del primer codigo de inicio
            "000000016742aabb" + "0000000168ce" + "00000165b80000030001" + // SPS, PPS, IDR (con 00 00 03 dentro)
            "00000001419a00" + // P; el cero final es relleno, no parte del NAL
            "000001419b02" + "0000000109f0", // P y un AUD que lo cierra
    )
    private val frames = listOf(
        "000000016742aabb0000000168ce0000000165b80000030001",
        "00000001419a",
        "00000001419b02",
    )

    private fun split(sizes: Sequence<Int>): List<String> {
        val out = ArrayList<String>()
        val s = AnnexB { d, n -> out.add(hex(d, n)) }
        var at = 0
        val it = sizes.iterator()
        while (at < stream.size) {
            val n = minOf(it.next(), stream.size - at)
            s.feed(stream.copyOfRange(at, at + n))
            at += n
        }
        return out
    }

    @Test
    fun annexB() {
        assertEquals(frames, split(sequenceOf(stream.size)))
        // Todos los cortes posibles en dos trozos (incluye partir cada codigo de inicio) y trozos fijos.
        for (cut in 1 until stream.size) assertEquals("corte en $cut", frames, split(sequenceOf(cut, stream.size)))
        for (n in 1..7) assertEquals("trozos de $n", frames, split(generateSequence { n }))
    }

    @Test
    fun annexBLargeFrame() {
        val out = ArrayList<Int>()
        val s = AnnexB { _, n -> out.add(n) }
        val nal = ByteArray(300_000) { 0x65 }
        repeat(2) {
            s.feed(byteArrayOf(0, 0, 0, 1))
            s.feed(nal)
        }
        s.feed(byteArrayOf(0, 0, 1, 9))
        assertEquals(listOf(300_004, 300_004), out)
    }
}
