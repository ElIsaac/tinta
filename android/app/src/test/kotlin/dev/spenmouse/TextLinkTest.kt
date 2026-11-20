package dev.spenmouse

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Receptor falso en loopback para TEXT/TACK: reenvio hasta el TACK, orden y agrupado. */
class TextLinkTest {
    private val keyHex = "000102030405060708090a0b0c0d0e0f"
    private val key = hexToBytes(keyHex)
    private val session = 0x1122334455667788L
    private val sock = DatagramSocket(0, InetAddress.getLoopbackAddress())
    private val buf = ByteArray(2048)
    private val pkt = DatagramPacket(buf, buf.size)
    private var lastSeq = -1L

    private class Text(val seq: Long, val tid: Long, val plain: String, val at: Long)

    private fun tag(b: ByteArray, n: Int) = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(key, "HmacSHA256"))
        update(b, 0, n)
        doFinal().copyOf(8)
    }

    private fun u32(b: ByteArray, o: Int) = b.slice(o..o + 3).fold(0L) { a, x -> a shl 8 or (x.toLong() and 0xFF) }

    /** Siguiente TEXT descifrado como lo haria el receptor, o null. Contesta los HELLO; ignora STATE. */
    private fun text(ms: Int): Text? {
        val end = System.nanoTime() + ms * 1_000_000L
        while (true) {
            val left = (end - System.nanoTime()) / 1_000_000
            if (left <= 0) return null
            sock.soTimeout = left.toInt()
            try {
                sock.receive(pkt)
            } catch (e: SocketTimeoutException) {
                return null
            }
            val n = pkt.length
            val p = buf.copyOf(n)
            if (p[3].toInt() == 1) {
                p[3] = 2
                for (i in 0..7) p[8 + i] = (session ushr (56 - 8 * i)).toByte()
                System.arraycopy(tag(p, 16), 0, p, 16, 8)
                sock.send(DatagramPacket(p, 24, pkt.socketAddress))
            }
            if (p[3].toInt() != 4) continue
            assertEquals(listOf<Byte>(0x53, 0x4D, 1, 4), p.slice(0..3))
            assertEquals(listOf<Byte>(0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88.toByte()), p.slice(8..15))
            val seq = u32(p, 4)
            assertTrue("seq creciente", seq > lastSeq)
            lastSeq = seq
            val plain = newAead().run { // falla (AEADBadTagException) si el cifrado no cumple el contrato
                init(Cipher.DECRYPT_MODE, SecretKeySpec(deriveKey(key, "spenmouse text"), "ChaCha20"), IvParameterSpec(p.copyOfRange(8, 16) + p.copyOfRange(4, 8)))
                updateAAD(p, 0, 20)
                doFinal(p, 20, n - 20)
            }
            return Text(seq, u32(p, 16), String(plain), System.nanoTime())
        }
    }

    private fun tack(t: Text, corrupt: Boolean = false) {
        val p = ByteArray(TACK_LEN)
        System.arraycopy(buf, 0, p, 0, 20) // cabecera (mismo seq), session y tid del TEXT recien leido
        p[3] = 5
        System.arraycopy(tag(p, 20), 0, p, 20, 8)
        if (corrupt) p[27] = (p[27].toInt() xor 1).toByte()
        sock.send(DatagramPacket(p, p.size, pkt.socketAddress))
        assertEquals(t.seq, u32(p, 4))
    }

    @Test
    fun stopAndWait() {
        val link = Link(Pairing("127.0.0.1", sock.localPort, keyHex, "t")) {}
        link.padW = 1500
        link.padH = 700
        link.sendText("Hola ") // antes de conectar: espera en cola
        link.start()

        val a = text(2000)!!
        assertEquals("Hola ", a.plain)
        // Sin TACK: lo nuevo se encola y el mismo tid se reenvia cada 150 ms con seq y cifrado nuevos.
        link.sendText("ñ")
        link.sendText("\b")
        link.sendText("\n")
        val b = text(1000)!!
        assertEquals(a.tid, b.tid)
        assertEquals("Hola ", b.plain)
        assertTrue("reenvio a los ~150 ms", (b.at - a.at) in 100_000_000L..400_000_000L)
        // Un TACK con tag malo no cuenta.
        tack(b, corrupt = true)
        val c = text(1000)!!
        assertEquals(a.tid, c.tid)
        assertEquals("Hola ", c.plain)

        // Con el TACK sale de inmediato el siguiente tid con todo lo encolado, en orden.
        tack(c)
        val d = text(100)!!
        assertEquals(a.tid + 1, d.tid)
        assertEquals("ñ\b\n", d.plain)
        tack(d)
        assertNull("nada pendiente", text(400))

        // Un TACK viejo repetido no confirma el texto siguiente.
        link.sendText("x")
        val e = text(1000)!!
        assertEquals(d.tid + 1, e.tid)
        val old = ByteArray(TACK_LEN)
        System.arraycopy(buf, 0, old, 0, 20)
        old[3] = 5
        old[19] = (old[19] - 1).toByte()
        System.arraycopy(tag(old, 20), 0, old, 20, 8)
        sock.send(DatagramPacket(old, old.size, pkt.socketAddress))
        val f = text(1000)!!
        assertEquals(e.tid, f.tid)
        tack(f)
        assertNull(text(400))
        link.stop()
        sock.close()
    }

    @Test
    fun wsListVector() {
        val key = ByteArray(16) { it.toByte() }
        val pkt = hexToBytes("534d0107000000091122334455667788b3adba958de12e533cda79eb6a5b849b19c32119d93d01d26c0224c54d0c7673c8ada9d01af0016284")
        val codec = Codec(key)
        assertEquals("11\tHDMI-A-1\tkitty: vpn 1\n", codec.parseWsList(pkt, pkt.size, 0x1122334455667788L))
        assertNull(codec.parseWsList(pkt, pkt.size, 1L))
        pkt[20] = (pkt[20].toInt() xor 1).toByte()
        assertNull(codec.parseWsList(pkt, pkt.size, 0x1122334455667788L))
    }

    @Test
    fun parsesWorkspaceList() {
        val ws = parseWorkspaces("2\teDP-1\t0,0,1920,1200\t55a,12,48,343,563,kitty: vpn 1\n11\tspenmouse\t3840,0,1560,628\t*55b,3852,48,700,500,firefox: Docs, v2\tbad\n\nrota\n")
        assertEquals(2, ws.size)
        assertEquals("2", ws[0].id)
        assertEquals(1920, ws[0].mw)
        assertEquals("kitty", ws[0].windows[0].app)
        assertEquals(343, ws[0].windows[0].w)
        assertEquals("spenmouse", ws[1].monitor)
        assertEquals(1, ws[1].windows.size)
        assertEquals("firefox: Docs, v2", ws[1].windows[0].label)
        assertEquals("55a", ws[0].windows[0].addr)
        assertTrue(ws[1].windows[0].lent && !ws[0].windows[0].lent)
        assertEquals("Docs, v2", ws[1].windows[0].title)
    }

    @Test
    fun magicPacketLayout() {
        val m = magicPacket("02:11:22:33:44:55")!!
        assertEquals(102, m.size)
        assertTrue(m.take(6).all { it == 0xFF.toByte() })
        assertEquals(0x02.toByte(), m[6])
        assertEquals(0x55.toByte(), m[101])
        assertNull(magicPacket("nope"))
        assertNull(magicPacket("02:11:22:33:44"))
    }

    @Test
    fun pairingAlternates() {
        val p = Pairing.parse("spenmouse://pair?host=192.168.0.28&port=47800&key=000102030405060708090a0b0c0d0e0f&name=pc&alt=100.64.0.7,nope,10.0.0.5")
        assertEquals(listOf("100.64.0.7", "10.0.0.5"), p.alt)
        assertTrue(Pairing.parse("spenmouse://pair?host=192.168.0.28&key=000102030405060708090a0b0c0d0e0f").alt.isEmpty())
    }
}
