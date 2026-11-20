package dev.spenmouse

import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import android.view.Surface
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket

private const val TAG = "spenmouse"
private const val CONNECT_MS = 2000
private const val RETRY_MS = 500L
private const val INPUT_WAIT_US = 10_000L
private const val OUTPUT_WAIT_US = 50_000L
private const val MAX_INPUT = 4 shl 20
private const val LOG_EVERY = 300
private const val MAX_W = 4096 // el receptor cambia de monitor (y de resolucion) sin reconectar
private const val MAX_H = 2304

/**
 * Modo pantalla: un hilo conecta el TCP, descifra y alimenta al decodificador; otro saca cuadros y
 * pinta siempre el mas nuevo. Reconecta solo hasta [stop]. [onSize] llega en un hilo de fondo.
 */
class Screen(
    private val pairing: Pairing,
    private val link: Link,
    private val surface: Surface,
    private val w: Int,
    private val h: Int,
    private val onSize: (Int, Int) -> Unit,
) {
    @Volatile private var running = true
    @Volatile private var sock: Socket? = null
    private val thread = Thread(::run, "spen-video").apply { start() }

    /** Vuelve con el decodificador ya liberado (o tras 1 s): la Surface puede destruirse despues. */
    fun stop() {
        running = false
        try {
            sock?.close()
        } catch (e: IOException) {
        }
        thread.interrupt()
        thread.join(1000)
    }

    private fun run() {
        while (running) {
            try {
                session()
            } catch (e: Exception) {
                if (running) Log.i(TAG, "video: $e")
            }
            if (running) try {
                Thread.sleep(RETRY_MS)
            } catch (e: InterruptedException) {
            }
        }
    }

    private fun session() {
        val s = Socket()
        sock = s
        try {
            if (!running) return
            s.tcpNoDelay = true
            s.connect(InetSocketAddress(link.host, pairing.port), CONNECT_MS)
            // El seq se pide ya conectado: un STATE posterior que llegue antes invalida el STREAM.
            s.getOutputStream().write(link.streamRequest(w, h) ?: return)
            val records = RecordReader(hexToBytes(pairing.key), s.getInputStream())
            val first = records.next() // sin respuesta valida no vale la pena crear el decodificador
            decode(records, first)
        } finally {
            s.close()
        }
    }

    private fun decode(records: RecordReader, first: ByteArray) {
        val codec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        var drain: Thread? = null
        try {
            // El tamano real llega en el SPS; este solo es una pista inicial.
            codec.configure(
                MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h).apply {
                    setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                    setInteger(MediaFormat.KEY_MAX_WIDTH, MAX_W)
                    setInteger(MediaFormat.KEY_MAX_HEIGHT, MAX_H)
                    setInteger(MediaFormat.KEY_PRIORITY, 0)
                    setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, MAX_INPUT)
                },
                surface, null, 0,
            )
            codec.start()
            drain = Thread({ drain(codec) }, "spen-video-out").apply { start() }
            val split = AnnexB { data, len ->
                var i: Int
                do i = codec.dequeueInputBuffer(INPUT_WAIT_US) while (i < 0 && running)
                if (i >= 0) {
                    val b = codec.getInputBuffer(i)!!
                    if (len > b.capacity()) throw IOException("cuadro de $len bytes")
                    b.put(data, 0, len)
                    codec.queueInputBuffer(i, 0, len, System.nanoTime() / 1000, 0)
                }
            }
            split.feed(first)
            while (running) split.feed(records.next())
        } finally {
            try {
                drain?.interrupt()
                drain?.join()
            } catch (e: InterruptedException) {
                // stop() interrumpe este hilo; el otro sale solo al fallar su siguiente llamada
            }
            codec.release()
        }
    }

    private fun drain(codec: MediaCodec) {
        val info = MediaCodec.BufferInfo()
        var frames = 0
        fun size() {
            val f = codec.outputFormat
            val crop = f.containsKey("crop-right") && f.containsKey("crop-bottom")
            val vw = if (crop) f.getInteger("crop-right") + 1 - f.getInteger("crop-left") else f.getInteger(MediaFormat.KEY_WIDTH)
            val vh = if (crop) f.getInteger("crop-bottom") + 1 - f.getInteger("crop-top") else f.getInteger(MediaFormat.KEY_HEIGHT)
            Log.i(TAG, "video: formato ${vw}x$vh")
            onSize(vw, vh)
        }
        try {
            while (!Thread.interrupted()) {
                var i = codec.dequeueOutputBuffer(info, OUTPUT_WAIT_US)
                if (i == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) size()
                if (i < 0) continue
                while (true) { // sin cola: si ya hay cuadros mas nuevos, los viejos no se pintan
                    val j = codec.dequeueOutputBuffer(info, 0)
                    if (j == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                        size()
                        continue
                    }
                    if (j < 0) break
                    codec.releaseOutputBuffer(i, false)
                    i = j
                }
                codec.releaseOutputBuffer(i, true)
                if (frames++ % LOG_EVERY == 0) Log.i(TAG, "video: $frames cuadros pintados")
            }
        } catch (e: IllegalStateException) {
            // el decodificador fallo o se libero: la sesion termina por el otro hilo
        }
    }
}
