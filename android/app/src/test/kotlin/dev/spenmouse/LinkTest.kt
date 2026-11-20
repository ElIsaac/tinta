package dev.spenmouse

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Receptor falso en loopback: comprueba HELLO/WELCOME, latido, 3x inactivo y el cierre. */
class LinkTest {
    private val key = hexToBytes("000102030405060708090a0b0c0d0e0f")
    private val session = 0x1122334455667788L
    private val sock = DatagramSocket(0, InetAddress.getLoopbackAddress())
    private val buf = ByteArray(64)
    private val pkt = DatagramPacket(buf, buf.size)

    private fun tag(n: Int) = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(key, "HmacSHA256"))
        update(buf, 0, n)
        doFinal().copyOf(8)
    }

    /** Siguiente paquete valido, o null si no llega nada en [ms]. Contesta los HELLO. */
    private fun next(ms: Int): ByteArray? {
        sock.soTimeout = ms
        try {
            sock.receive(pkt)
        } catch (e: SocketTimeoutException) {
            return null
        }
        val n = pkt.length
        assertTrue("tag", tag(n - 8).contentEquals(buf.copyOfRange(n - 8, n)))
        val p = buf.copyOf(n)
        if (p[3].toInt() == 1) {
            assertEquals(HELLO_LEN, n)
            assertEquals(listOf<Byte>(0x05, 0xDC.toByte(), 0x02, 0xBC.toByte()), p.slice(12..15))
            buf[3] = 2
            for (i in 0..7) buf[8 + i] = (session ushr (56 - 8 * i)).toByte()
            System.arraycopy(tag(16), 0, buf, 16, 8)
            sock.send(DatagramPacket(buf, 24, pkt.socketAddress))
        }
        return p
    }

    private fun seq(p: ByteArray) = p.slice(4..7).fold(0L) { a, b -> a shl 8 or (b.toLong() and 0xFF) }

    /** Siguiente STATE (saltando HELLOs), o null. */
    private fun state(ms: Int): ByteArray? {
        while (true) {
            val p = next(ms) ?: return null
            if (p[3].toInt() == 3) return p
        }
    }

    @Test
    fun session() {
        val events = ArrayList<Boolean>()
        val link = Link(Pairing("127.0.0.1", sock.localPort, "000102030405060708090a0b0c0d0e0f", "t")) {
            synchronized(events) { events.add(it) }
        }
        link.padW = 1500
        link.padH = 700
        link.update(IN_RANGE or CONTACT, 100, 90, 0, 5) // antes de conectar: no debe salir STATE todavia
        link.start()

        val hello = next(2000)!!
        assertEquals(1, hello[3].toInt())
        // Al llegar el WELCOME se manda el estado actual, y sigue el latido aunque nada cambie.
        var last = seq(hello)
        val t0 = System.nanoTime()
        repeat(6) {
            val s = state(500)!!
            assertEquals(STATE_LEN, s.size)
            assertEquals(IN_RANGE or CONTACT, s[16].toInt())
            assertEquals(listOf<Byte>(0, 100, 0, 90, 0, 0, 0, 5), s.slice(17..24))
            assertTrue("seq creciente", seq(s) > last)
            last = seq(s)
        }
        assertTrue("latido <= 50 ms", (System.nanoTime() - t0) / 5 < 50_000_000)
        assertEquals(listOf(true), synchronized(events) { events.toList() })

        // Todo inactivo: el estado final sale exactamente 3 veces y luego silencio.
        link.update(0, 100, 90, 0, 5)
        var idle = 0
        while (true) {
            val s = state(300) ?: break
            if (s[16].toInt() == 0) idle++ else assertEquals("latido tardio", 0, idle)
        }
        assertEquals(3, idle)

        // Al pasar a segundo plano con algo activo: 3 estados inactivos y nada mas (ni HELLO).
        link.update(BTN_LEFT, 1, 2, 0, 5)
        assertEquals(BTN_LEFT, state(300)!![16].toInt())
        link.stop()
        idle = 0
        while (true) {
            val p = next(1500) ?: break
            assertEquals(3, p[3].toInt())
            if (p[16].toInt() == 0) idle++
        }
        assertEquals(3, idle)
        sock.close()
    }
}
