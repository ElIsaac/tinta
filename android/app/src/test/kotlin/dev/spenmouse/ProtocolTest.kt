package dev.spenmouse

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ProtocolTest {
    private val codec = Codec(hexToBytes("000102030405060708090a0b0c0d0e0f"))
    private val buf = ByteArray(64)
    private val welcome = "534d0102000000011122334455667788214588e1a8d5a9ae"

    private fun hex(n: Int) = buf.take(n).joinToString("") { "%02x".format(it) }

    @Test
    fun helloVector() {
        val n = codec.encodeHello(buf, 1, 0x01020304, 1500, 700)
        assertEquals("534d0101000000010102030405dc02bc8d387c9b70989305", hex(n))
    }

    @Test
    fun stateVector() {
        val n = codec.encodeState(buf, 2, 0x1122334455667788, 0x07, 32768, 16384, 40000, -3)
        assertEquals("534d010300000002112233445566778807800040009c40fffd152ce5b8b9bbe8d8", hex(n))
    }

    @Test
    fun welcomeVector() {
        val b = hexToBytes(welcome)
        assertEquals(0x1122334455667788, codec.parseWelcome(b, b.size))
        assertEquals(1, codec.seqOf(b))
    }

    @Test
    fun welcomeRejectsBadPackets() {
        for (i in 0 until WELCOME_LEN) {
            val b = hexToBytes(welcome)
            b[i] = (b[i].toInt() xor 1).toByte()
            assertEquals("byte $i alterado", 0L, codec.parseWelcome(b, b.size))
        }
        val b = hexToBytes(welcome)
        assertEquals(0L, codec.parseWelcome(b, b.size - 1))
        assertEquals(0L, Codec(ByteArray(16)).parseWelcome(b, b.size))
    }

    @Test
    fun pairingUri() {
        val p = Pairing.parse(" spenmouse://pair?host=192.168.0.28&port=47800&key=000102030405060708090a0b0c0d0e0f&name=laptop\n")
        assertEquals("192.168.0.28", p.host)
        assertEquals(47800, p.port)
        assertEquals("000102030405060708090a0b0c0d0e0f", p.key)
        assertEquals("laptop", p.name)
        assertEquals(listOf<Byte>(192.toByte(), 168.toByte(), 0, 28), p.hostBytes.toList())

        for (bad in listOf(
            "http://pair?host=1.2.3.4&port=1&key=000102030405060708090a0b0c0d0e0f",
            "spenmouse://pair?host=1.2.3.4&port=47800&key=000102030405060708090a0b0c0d0e",
            "spenmouse://pair?host=1.2.3.4&port=47800&key=000102030405060708090a0b0c0d0e0f00",
            "spenmouse://pair?host=1.2.3.4&port=47800&key=g00102030405060708090a0b0c0d0e0f",
            "spenmouse://pair?host=1.2.3.4&port=70000&key=000102030405060708090a0b0c0d0e0f",
            "spenmouse://pair?host=1.2.3.4&port=0&key=000102030405060708090a0b0c0d0e0f",
            "spenmouse://pair?host=1.2.3.4&port=x&key=000102030405060708090a0b0c0d0e0f",
            "spenmouse://pair?host=1.2.3.456&port=47800&key=000102030405060708090a0b0c0d0e0f",
            "spenmouse://pair?host=laptop.local&port=47800&key=000102030405060708090a0b0c0d0e0f",
        )) assertThrows(bad, IllegalArgumentException::class.java) { Pairing.parse(bad) }
    }
}
