package dev.spenmouse

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.graphics.drawable.StateListDrawable
import android.view.HapticFeedbackConstants
import android.widget.ImageButton
import android.widget.ImageView

// Tema oscuro unico. Todo color de la interfaz sale de aqui.
const val C_BG = 0xFF0B0D12.toInt() // area tactil
const val C_SURFACE = 0xFF141821.toInt() // barra de controles
const val C_SURFACE_2 = 0xFF1F2430.toInt() // botones y campo de texto
const val C_BORDER = 0xFF2A3040.toInt()
const val C_TEXT = 0xFFE6E8EE.toInt()
const val C_MUTED = 0xFF9AA3B2.toInt()
const val C_PRIMARY = 0xFF818CF8.toInt()
const val C_PRIMARY_BG = 0x40818CF8
const val C_OK = 0xFF34D399.toInt()
const val C_WARN = 0xFFF59E0B.toInt()
private const val C_RIPPLE = 0x33FFFFFF
const val TARGET_DP = 48 // objetivo tactil minimo de Android
const val GAP_DP = 8
private const val RADIUS_DP = 12f

fun Context.rounded(color: Int): Drawable = GradientDrawable().apply {
    cornerRadius = RADIUS_DP * resources.displayMetrics.density
    setColor(color)
}

/** Fondo comun de los botones: redondeado, indigo translucido si esta seleccionado, onda al presionar. */
fun Context.keyBackground(): Drawable {
    val states = StateListDrawable().apply {
        addState(intArrayOf(android.R.attr.state_selected), rounded(C_PRIMARY_BG))
        addState(intArrayOf(), rounded(C_SURFACE_2))
    }
    return RippleDrawable(ColorStateList.valueOf(C_RIPPLE), states, rounded(C_TEXT))
}

private val KEY_COLORS = ColorStateList(
    arrayOf(intArrayOf(android.R.attr.state_selected), intArrayOf()),
    intArrayOf(C_PRIMARY, C_TEXT),
)

/** Algo que queda encendido o apagado (modos y modificadores). */
interface Toggle {
    var isOn: Boolean
}

/** Tecla con texto (Esc, Tab, Ctrl...). Con [onChange] es un interruptor; con [onTap], una pulsacion. */
class TextKey(context: Context, text: String, label: String, onTap: (() -> Unit)? = null, private val onChange: ((Boolean) -> Unit)? = null) :
    android.widget.TextView(context), Toggle {
    override var isOn = false
        set(value) {
            if (field == value) return
            field = value
            isSelected = value
            stateDescription = if (value) "activado" else "desactivado"
            onChange?.invoke(value)
        }

    init {
        this.text = text
        textSize = 12f
        typeface = android.graphics.Typeface.create("sans-serif-medium", android.graphics.Typeface.NORMAL)
        gravity = android.view.Gravity.CENTER
        setTextColor(KEY_COLORS)
        background = context.keyBackground()
        contentDescription = label
        tooltipText = label
        if (onChange != null) stateDescription = "desactivado"
        setOnClickListener {
            performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
            if (onChange != null) isOn = !isOn else onTap?.invoke()
        }
    }
}

/** Boton de icono de 48dp: fondo redondeado, onda al presionar, nombre accesible y tooltip. */
open class IconButton(context: Context, icon: Int, label: String, onTap: (() -> Unit)? = null) : ImageButton(context) {
    init {
        background = context.keyBackground()
        setImageResource(icon)
        scaleType = ImageView.ScaleType.CENTER
        imageTintList = KEY_COLORS
        contentDescription = label
        tooltipText = label
        if (onTap != null) setOnClickListener {
            performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
            onTap()
        }
    }
}

/** Boton de icono con estado: encendido cambia fondo y color, y se anuncia a lectores de pantalla. */
class IconToggle(context: Context, icon: Int, label: String, private val onChange: (Boolean) -> Unit) :
    IconButton(context, icon, label), Toggle {
    override var isOn = false
        set(value) {
            if (field == value) return
            field = value
            isSelected = value
            stateDescription = if (value) "activado" else "desactivado"
            onChange(value)
        }

    init {
        stateDescription = "desactivado"
        setOnClickListener {
            performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
            isOn = !isOn
        }
    }
}
