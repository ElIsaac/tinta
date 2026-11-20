package dev.spenmouse

import android.content.Context
import android.view.InputDevice
import android.view.MotionEvent
import android.view.View
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.roundToInt

private const val TAP_MAX_MS = 250L // duracion maxima de un toque para contar como tap
private const val TAP_SLOP_DP = 8f // movimiento maximo durante un tap
private const val TAP_DRAG_WINDOW_MS = 180L // BTN_LEFT queda abajo este tiempo tras un tap; volver a tocar dentro = arrastrar
private const val CLICK_MS = 60L // duracion del clic derecho y pausa entre los dos clics de un doble tap
private const val WHEEL_STEP_DP = 20f // recorrido vertical de dos dedos por paso de rueda
private const val WHEEL_DIR = 1 // 1: dedos hacia arriba = rueda arriba; -1 lo invierte
private const val PEN_OUT_GRACE_MS = 40L // tapa el hueco HOVER_EXIT -> DOWN y UP -> HOVER_ENTER
private const val MOBILE_STEP_DP = 14f // modo movil: recorrido del dedo por paso de rueda
private const val LONG_PRESS_MS = 450L // modo movil: dejar el dedo quieto = empezar a seleccionar
private const val PEN_HOLD_MS = 350L // boton lateral: soltar antes = clic izquierdo, sostener = clic derecho
private const val PEN_BUTTONS = MotionEvent.BUTTON_STYLUS_PRIMARY or MotionEvent.BUTTON_SECONDARY
private const val TENTHS_MM_PER_INCH = 254f

/** Area tactil: traduce MotionEvents al estado absoluto del contrato y se lo pasa al [Link]. */
class PadView(context: Context) : View(context) {
    /** Zona de (x,y): 0 = touchpad, 1 = franja de zoom, 2 = franja de paneo. Solo dedo. */
    var barAt: ((Float, Float) -> Int)? = null
    var onBar: ((Int, Float, Float) -> Unit)? = null
    private var barMode = 0
    private var barX = 0f
    private var barY = 0f

    /** Modo movil: el dedo y el lapiz apuntan donde tocan, dentro de [absRect] (el video). */
    var absolute: () -> Boolean = { false }
    var absRect: () -> android.graphics.Rect? = { null }
    private var absLive = false
    private var absLong = false
    private val mobileStep = MOBILE_STEP_DP * resources.displayMetrics.density
    private val absRelease = Runnable {
        btn = btn and BTN_LEFT.inv()
        tool = 0
        push()
    }
    private var absTwo = false
    // Como en un telefono: mantener presionado agarra el texto (boton izquierdo abajo) y arrastrar lo selecciona.
    private val longPress = Runnable {
        absLong = true
        moved = false
        performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
        btn = btn or BTN_LEFT
        push()
    }
    // Soltar sin haber arrastrado: dos clics rapidos = doble clic, que selecciona la palabra.
    // (La pulsacion larga no cuenta como primer clic: dura mas que el intervalo de doble clic.)
    private var wordClicks = 0
    private val wordPress = Runnable {
        btn = btn or BTN_LEFT
        push()
        postDelayed(wordRelease, CLICK_MS)
    }
    private val wordRelease: Runnable = Runnable {
        btn = btn and BTN_LEFT.inv()
        if (--wordClicks > 0) postDelayed(wordPress, CLICK_MS) else tool = 0
        push()
    }

    var link: Link? = null
        set(value) {
            field = value
            tool = 0
            btn = 0
            pressure = 0
            fingerLive = false
            dragging = false
            tapPending = false
            penBtnDown = false
            penHeld = false
            penTip = false
            reportSize()
        }

    private val slop = TAP_SLOP_DP * resources.displayMetrics.density
    private val wheelStep = WHEEL_STEP_DP * resources.displayMetrics.density

    // Estado que se envia. tool = IN_RANGE/CONTACT/TOOL_PEN, btn = BTN_*.
    // PEN_BUTTON no se envia: el boton lateral se interpreta aqui como gesto.
    private var tool = 0
    private var btn = 0
    private var x = 0
    private var y = 0
    private var pressure = 0
    private var wheel = 0

    // Gesto de dedo en curso.
    private var fingerLive = false
    private var downTime = 0L
    private var downX = 0f
    private var downY = 0f
    private var moved = false
    private var fingers = 0
    private var multi = false
    private var scrollStart = 0f
    private var scrollY = 0f
    private var dragging = false
    private var tapPending = false
    private var penBtnDown = false
    private var penHeld = false
    private var penTip = false

    private val penHold = Runnable {
        penHeld = true
        btn = btn or BTN_RIGHT
        push()
    }
    private val penOut = Runnable {
        tool = 0
        penTip(false)
        penButton(false) // si el lapiz se va con el boton abajo ya no llega el release
        pressure = 0
        push()
    }
    private val tapRelease = Runnable {
        tapPending = false
        btn = btn and BTN_LEFT.inv()
        push()
    }
    private val tapPress = Runnable {
        btn = btn or BTN_LEFT
        tapPending = true
        postDelayed(tapRelease, TAP_DRAG_WINDOW_MS)
        push()
    }
    private val rightRelease = Runnable {
        btn = btn and BTN_RIGHT.inv()
        push()
    }

    init {
        setBackgroundColor(0xFF101418.toInt())
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        // Sin esto las muestras (incluido hover) se agrupan al vsync.
        requestUnbufferedDispatch(InputDevice.SOURCE_CLASS_POINTER)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) = reportSize()

    private fun reportSize() {
        val dm = resources.displayMetrics
        link?.padW = (width / dm.xdpi * TENTHS_MM_PER_INCH).roundToInt().coerceIn(0, 65535)
        link?.padH = (height / dm.ydpi * TENTHS_MM_PER_INCH).roundToInt().coerceIn(0, 65535)
    }

    private fun push() {
        link?.update(tool or btn, x, y, pressure, wheel)
    }

    /** Posicion dentro del video, 0..65535. Fuera del video se recorta al borde. */
    private fun setAbs(px: Float, py: Float): Boolean {
        val r = absRect() ?: return false
        if (r.width() <= 0 || r.height() <= 0) return false
        x = ((px - r.left) / r.width() * 65535f).toInt().coerceIn(0, 65535)
        y = ((py - r.top) / r.height() * 65535f).toInt().coerceIn(0, 65535)
        return true
    }

    private fun setXY(px: Float, py: Float) {
        x = (px / width * 65535f).toInt().coerceIn(0, 65535)
        y = (py / height * 65535f).toInt().coerceIn(0, 65535)
    }

    private fun isPen(e: MotionEvent) = e.getToolType(0).let {
        it == MotionEvent.TOOL_TYPE_STYLUS || it == MotionEvent.TOOL_TYPE_ERASER
    }

    override fun onHoverEvent(e: MotionEvent): Boolean {
        if (isPen(e)) pen(e)
        return true
    }

    override fun onGenericMotionEvent(e: MotionEvent): Boolean {
        val a = e.actionMasked
        if ((a == MotionEvent.ACTION_BUTTON_PRESS || a == MotionEvent.ACTION_BUTTON_RELEASE) && isPen(e)) {
            pen(e)
            return true
        }
        return super.onGenericMotionEvent(e)
    }

    override fun onTouchEvent(e: MotionEvent): Boolean {
        if (e.actionMasked == MotionEvent.ACTION_DOWN) requestUnbufferedDispatch(e)
        if (isPen(e)) pen(e) else finger(e)
        return true
    }

    private fun pen(e: MotionEvent) {
        if (fingerLive) { // el lapiz manda: se abandona el gesto de dedo sin dejar el boton pegado
            fingerLive = false
            if (dragging) {
                dragging = false
                btn = btn and BTN_LEFT.inv()
            }
        }
        removeCallbacks(penOut)
        var f = IN_RANGE or TOOL_PEN
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_MOVE -> f = f or CONTACT
            MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE -> f = f or (tool and CONTACT)
            MotionEvent.ACTION_HOVER_EXIT, MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL ->
                postDelayed(penOut, PEN_OUT_GRACE_MS)
        }
        penButton(e.buttonState and PEN_BUTTONS != 0)
        // En modo movil el lapiz es una tableta absoluta: la punta ya hace clic, no se suma BTN_LEFT.
        val abs = absolute() && setAbs(e.x, e.y)
        if (abs) f = f or PEN_ABS
        penTip(f and CONTACT != 0 && !abs)
        tool = f
        if (!abs) setXY(e.x, e.y)
        pressure = if (f and CONTACT != 0) (e.pressure * 65535f).toInt().coerceIn(0, 65535) else 0
        push()
    }

    // Lapiz relativo: el receptor no convierte el contacto en clic, la punta es BTN_LEFT.
    private fun penTip(down: Boolean) {
        if (down == penTip) return
        penTip = down
        removeCallbacks(tapRelease)
        btn = if (down) btn or BTN_LEFT else btn and BTN_LEFT.inv()
    }

    private fun penButton(down: Boolean) {
        if (down == penBtnDown) return
        penBtnDown = down
        removeCallbacks(penHold)
        if (down) {
            postDelayed(penHold, PEN_HOLD_MS)
        } else if (penHeld) {
            penHeld = false
            btn = btn and BTN_RIGHT.inv()
        } else if (tool != 0) { // pulsacion corta con el lapiz aun en rango
            btn = btn or BTN_LEFT
            postDelayed(tapRelease, CLICK_MS)
        }
    }

    private fun avgY(e: MotionEvent) = (e.getY(0) + e.getY(1)) / 2

    private fun finger(e: MotionEvent) {
        val a = e.actionMasked
        if (a == MotionEvent.ACTION_DOWN) barMode = barAt?.invoke(e.x, e.y) ?: 0
        if (barMode != 0) { // el dedo empezo en una franja: mueve el video, no el cursor
            if (a == MotionEvent.ACTION_MOVE) onBar?.invoke(barMode, e.x - barX, e.y - barY)
            barX = e.x
            barY = e.y
            if (a == MotionEvent.ACTION_UP || a == MotionEvent.ACTION_CANCEL) barMode = 0
            return
        }
        // Modo movil: el dedo se traduce a cursor + rueda, porque muchas apps de escritorio (kitty)
        // ignoran los toques reales. Tocar lleva el cursor a ese punto; soltar sin mover = clic;
        // arrastrar = scroll natural; dejarlo quieto = clic derecho.
        if (a == MotionEvent.ACTION_DOWN) absLive = absolute() && tool and TOOL_PEN == 0 && setAbs(e.x, e.y)
        if (absLive) {
            when (a) {
                MotionEvent.ACTION_DOWN -> {
                    removeCallbacks(absRelease)
                    removeCallbacks(wordPress)
                    removeCallbacks(wordRelease)
                    btn = btn and BTN_LEFT.inv()
                    downX = e.x
                    downY = e.y
                    scrollY = e.y
                    moved = false
                    absLong = false
                    absTwo = false
                    tool = IN_RANGE or TOOL_PEN or PEN_ABS // como un lapiz flotando: mueve el cursor sin tocar
                    pressure = 0
                    postDelayed(longPress, LONG_PRESS_MS)
                }
                MotionEvent.ACTION_POINTER_DOWN -> { // segundo dedo: sera un clic derecho si no hay arrastre
                    removeCallbacks(longPress)
                    if (absLong) return
                    absTwo = true
                    return
                }
                MotionEvent.ACTION_MOVE -> {
                    if (!moved && hypot(e.x - downX, e.y - downY) > slop) {
                        moved = true
                        removeCallbacks(longPress)
                        scrollY = e.y
                    }
                    if (absLong) {
                        // seleccionando: el cursor sigue al dedo con el boton abajo
                        if (!moved || !setAbs(e.x, e.y)) return
                    } else {
                        if (!moved || absTwo) return
                        val steps = ((scrollY - e.y) / mobileStep).toInt()
                        if (steps == 0) return
                        scrollY -= steps * mobileStep
                        wheel = (wheel - steps).toShort().toInt() // dedo hacia arriba = el contenido sube
                    }
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    removeCallbacks(longPress)
                    absLive = false
                    val up = a == MotionEvent.ACTION_UP
                    when {
                        absLong -> { // fin de la seleccion; sin arrastre, doble clic para tomar la palabra
                            btn = btn and BTN_LEFT.inv()
                            if (up && !moved) {
                                wordClicks = 2
                                postDelayed(wordPress, CLICK_MS)
                            } else tool = 0
                        }
                        up && absTwo && !moved -> { // toque con dos dedos = clic derecho
                            btn = btn or BTN_RIGHT
                            postDelayed(rightRelease, CLICK_MS)
                            tool = 0
                        }
                        up && !moved -> {
                            btn = btn or BTN_LEFT
                            postDelayed(absRelease, CLICK_MS)
                        }
                        else -> tool = 0
                    }
                }
                else -> return
            }
            push()
            return
        }
        // Con el lapiz en rango se ignora el gesto de dedo completo (palma apoyada).
        if (a == MotionEvent.ACTION_DOWN) fingerLive = tool and TOOL_PEN == 0
        if (!fingerLive) return
        when (a) {
            MotionEvent.ACTION_DOWN -> {
                downTime = e.eventTime
                downX = e.x
                downY = e.y
                moved = false
                multi = false
                fingers = 1
                if (tapPending) { // tap y volver a tocar: BTN_LEFT sigue abajo para arrastrar
                    removeCallbacks(tapRelease)
                    tapPending = false
                    dragging = true
                }
                tool = IN_RANGE or CONTACT
                pressure = 0
                setXY(e.x, e.y)
            }
            MotionEvent.ACTION_POINTER_DOWN -> {
                // Desde aqui x,y quedan congelados hasta levantar todo: el receptor no mueve el cursor.
                multi = true
                fingers = maxOf(fingers, e.pointerCount)
                scrollStart = avgY(e)
                scrollY = scrollStart
            }
            MotionEvent.ACTION_MOVE -> if (!multi) {
                if (hypot(e.x - downX, e.y - downY) > slop) moved = true
                setXY(e.x, e.y)
            } else if (fingers == 2 && e.pointerCount == 2) {
                val ay = avgY(e)
                if (abs(ay - scrollStart) > slop) moved = true
                val steps = ((scrollY - ay) / wheelStep).toInt()
                if (steps == 0) return
                scrollY -= steps * wheelStep
                wheel = (wheel + steps * WHEEL_DIR).toShort().toInt()
            } else return
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                fingerLive = false
                tool = 0
                val tap = a == MotionEvent.ACTION_UP && !moved && e.eventTime - downTime < TAP_MAX_MS
                if (dragging) {
                    dragging = false
                    btn = btn and BTN_LEFT.inv()
                    if (tap) postDelayed(tapPress, CLICK_MS) // era un doble tap: segundo clic
                } else if (tap && fingers == 1) {
                    tapPress.run()
                    return
                } else if (tap && fingers == 2) {
                    btn = btn or BTN_RIGHT
                    postDelayed(rightRelease, CLICK_MS)
                }
            }
            else -> return
        }
        push()
    }
}
