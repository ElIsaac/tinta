package dev.spenmouse

import android.app.Dialog
import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.text.TextUtils
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView

private const val PHONE_MONITOR = "spenmouse"
private const val SHEET_RADIUS_DP = 20f
private const val CARD_MIN_DP = 150 // ancho minimo de una tarjeta: decide cuantas columnas caben
private const val PREVIEW_MAX_RATIO = 1.4f // alto maximo de la miniatura respecto a su ancho (monitor vertical)
private const val PREVIEW_MIN_RATIO = 0.4f
private const val SHEET_MAX_HEIGHT = 0.8f // de la pantalla
private const val C_WINDOW_EDGE = 0x669AA3B2 // C_MUTED al 40%

/** Miniatura del acomodo: cada ventana como un bloque en su posicion real dentro del monitor. */
private class WsPreview(context: Context, private val ws: Workspace) : View(context) {
    private val dp = resources.displayMetrics.density
    // un tono mas claro que la tarjeta, para que cada ventana se distinga del fondo de la miniatura
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = C_BORDER }
    private val edge = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = C_WINDOW_EDGE
        style = Paint.Style.STROKE
        strokeWidth = dp
    }
    private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = C_MUTED
        textSize = 10 * resources.displayMetrics.scaledDensity
        textAlign = Paint.Align.CENTER
    }

    init {
        background = context.rounded(C_BG)
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO // la tarjeta ya lo describe con texto
    }

    override fun onMeasure(wSpec: Int, hSpec: Int) {
        val w = MeasureSpec.getSize(wSpec)
        val ratio = if (ws.mw > 0) (ws.mh.toFloat() / ws.mw).coerceIn(PREVIEW_MIN_RATIO, PREVIEW_MAX_RATIO) else 0.6f
        setMeasuredDimension(w, (w * ratio).toInt())
    }

    override fun onDraw(c: Canvas) {
        if (ws.mw <= 0 || ws.mh <= 0) return
        val k = minOf(width.toFloat() / ws.mw, height.toFloat() / ws.mh)
        val ox = (width - ws.mw * k) / 2
        val oy = (height - ws.mh * k) / 2
        val r = 3 * dp
        val gap = dp
        for (win in ws.windows) {
            val l = (ox + (win.x - ws.mx) * k + gap).coerceIn(0f, width.toFloat())
            val t = (oy + (win.y - ws.my) * k + gap).coerceIn(0f, height.toFloat())
            val rr = (ox + (win.x - ws.mx + win.w) * k - gap).coerceIn(0f, width.toFloat())
            val b = (oy + (win.y - ws.my + win.h) * k - gap).coerceIn(0f, height.toFloat())
            if (rr - l < 2 * dp || b - t < 2 * dp) continue
            c.drawRoundRect(l, t, rr, b, r, r, fill)
            c.drawRoundRect(l, t, rr, b, r, r, edge)
            val name = TextUtils.ellipsize(win.app, android.text.TextPaint(text), rr - l - 4 * dp, TextUtils.TruncateAt.END)
            if (name.length >= 2 && b - t > text.textSize * 1.4f) {
                c.drawText(name, 0, name.length, (l + rr) / 2, (t + b) / 2 - (text.ascent() + text.descent()) / 2, text)
            }
        }
    }
}

/**
 * Hoja inferior con una tarjeta por workspace: miniatura, numero, donde esta y sus ventanas.
 * Tocar la parte de arriba de una tarjeta trae el workspace entero ([onWorkspace]); tocar una
 * ventana trae solo esa ([onWindow]). Con [canLend] en falso las tarjetas se ven pero no actuan.
 */
fun Context.showWorkspacePanel(
    list: List<Workspace>,
    canLend: Boolean,
    widthPx: Int,
    onWorkspace: (Workspace) -> Unit,
    onWindow: (WsWindow) -> Unit,
): Dialog {
    val dp = resources.displayMetrics.density
    val gap = (GAP_DP * dp).toInt()
    val dialog = Dialog(this, android.R.style.Theme_DeviceDefault_Dialog_NoActionBar)

    fun label(size: Float, color: Int, lines: Int = 1) = TextView(this).apply {
        textSize = size
        setTextColor(color)
        maxLines = lines
        ellipsize = TextUtils.TruncateAt.END
    }

    fun ripple() = RippleDrawable(ColorStateList.valueOf(0x33FFFFFF), null, rounded(C_TEXT))
    fun tap(v: View, action: () -> Unit) = v.setOnClickListener {
        v.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
        dialog.dismiss()
        action()
    }

    // Tarjeta: arriba (miniatura + numero) trae el workspace entero; cada fila de abajo, solo esa ventana.
    fun card(ws: Workspace): View {
        val onPhone = ws.monitor == PHONE_MONITOR
        val where = if (onPhone) "En el teléfono" else ws.monitor
        val all = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = ripple()
            addView(WsPreview(this@showWorkspacePanel, ws))
            addView(LinearLayout(this@showWorkspacePanel).apply {
                gravity = Gravity.CENTER_VERTICAL
                minimumHeight = (TARGET_DP * dp).toInt()
                addView(label(18f, C_TEXT).apply { text = ws.id; typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL) })
                addView(label(12f, if (onPhone) C_PRIMARY else C_MUTED).apply { text = where; setPadding(gap, 0, gap, 0) }, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
                if (canLend) addView(label(12f, C_PRIMARY).apply { text = if (onPhone) "Devolver" else "Traer todo" })
            })
            contentDescription = "Workspace ${ws.id}, $where. " + if (onPhone) "Toca para devolverlo" else "Toca para traerlo completo"
            isEnabled = canLend
            if (canLend) tap(this) { onWorkspace(ws) }
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(gap, gap, gap, gap / 2)
            background = GradientDrawable().apply {
                cornerRadius = 12 * dp
                setColor(C_SURFACE_2)
                if (onPhone) setStroke((2 * dp).toInt(), C_PRIMARY)
            }
            alpha = if (canLend) 1f else 0.5f
            addView(all)
            if (ws.windows.isEmpty()) addView(label(12f, C_MUTED).apply { text = "Vacío"; setPadding(0, 0, 0, gap) })
            for (win in ws.windows) {
                // En el workspace del telefono solo actuan las ventanas traidas por separado (para devolverlas).
                val active = canLend && (!onPhone || win.lent)
                addView(View(this@showWorkspacePanel).apply { setBackgroundColor(C_BORDER) }, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 1))
                addView(LinearLayout(this@showWorkspacePanel).apply {
                    orientation = LinearLayout.VERTICAL
                    gravity = Gravity.CENTER_VERTICAL
                    minimumHeight = (TARGET_DP * dp).toInt()
                    setPadding(0, gap / 2, 0, gap / 2)
                    background = ripple()
                    addView(label(13f, C_TEXT).apply { text = win.app })
                    addView(label(12f, if (win.lent) C_PRIMARY else C_MUTED).apply {
                        text = if (win.lent) "Traída sola · toca para devolver" else win.title.ifEmpty { "Sin título" }
                    })
                    contentDescription = "Ventana ${win.label}. " + when {
                        win.lent -> "Toca para devolverla a su workspace"
                        active -> "Toca para traer solo esta ventana"
                        else -> ""
                    }
                    isEnabled = active
                    if (active) tap(this) { onWindow(win) }
                })
            }
        }
    }

    val columns = maxOf(2, ((widthPx - 2 * gap) / (CARD_MIN_DP * dp)).toInt())
    val grid = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
    list.chunked(columns).forEach { rowItems ->
        grid.addView(LinearLayout(this).apply {
            rowItems.forEach { addView(card(it), LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { setMargins(gap / 2, gap / 2, gap / 2, gap / 2) }) }
            // la ultima fila mantiene el ancho de columna
            repeat(columns - rowItems.size) { addView(View(this@showWorkspacePanel), LinearLayout.LayoutParams(0, 0, 1f).apply { setMargins(gap / 2, 0, gap / 2, 0) }) }
        })
    }

    val sheet = LinearLayout(this).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(gap + gap / 2, 2 * gap, gap + gap / 2, 2 * gap)
        background = GradientDrawable().apply {
            val r = SHEET_RADIUS_DP * dp
            cornerRadii = floatArrayOf(r, r, r, r, 0f, 0f, 0f, 0f)
            setColor(C_SURFACE)
        }
        addView(LinearLayout(this@showWorkspacePanel).apply {
            gravity = Gravity.CENTER_VERTICAL
            setPadding(gap / 2, 0, gap / 2, gap)
            addView(LinearLayout(this@showWorkspacePanel).apply {
                orientation = LinearLayout.VERTICAL
                addView(label(18f, C_TEXT).apply { text = "Workspaces"; typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL) })
                addView(label(13f, if (canLend) C_MUTED else C_WARN, 2).apply {
                    text = when {
                        list.isEmpty() -> "No hay workspaces con ventanas."
                        canLend -> "Toca la miniatura para traer el workspace entero, o una ventana para traer solo esa."
                        else -> "Enciende Pantalla para poder traerlos."
                    }
                })
            }, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
            addView(TextView(this@showWorkspacePanel).apply {
                text = "Cerrar"
                textSize = 14f
                setTextColor(C_PRIMARY)
                gravity = Gravity.CENTER
                minWidth = (TARGET_DP * dp).toInt()
                minHeight = (TARGET_DP * dp).toInt()
                setPadding(2 * gap, 0, 2 * gap, 0)
                background = RippleDrawable(ColorStateList.valueOf(0x33FFFFFF), null, rounded(C_TEXT))
                setOnClickListener { dialog.dismiss() }
            })
        })
        addView(ScrollView(this@showWorkspacePanel).apply { addView(grid) })
    }

    dialog.setContentView(sheet)
    dialog.setCanceledOnTouchOutside(true)
    dialog.window?.apply {
        setBackgroundDrawable(ColorDrawable(0))
        setGravity(Gravity.BOTTOM)
        setLayout(WindowManager.LayoutParams.MATCH_PARENT, WindowManager.LayoutParams.WRAP_CONTENT)
        attributes = attributes.apply { dimAmount = 0.5f }
        addFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND)
    }
    // tope de alto: la hoja nunca tapa toda la pantalla
    sheet.post {
        val max = (resources.displayMetrics.heightPixels * SHEET_MAX_HEIGHT).toInt()
        if (sheet.height > max) dialog.window?.setLayout(WindowManager.LayoutParams.MATCH_PARENT, max)
    }
    dialog.show()
    return dialog
}
