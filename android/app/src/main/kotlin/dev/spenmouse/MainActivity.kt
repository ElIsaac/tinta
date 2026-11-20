package dev.spenmouse

import android.app.Activity
import android.app.AlertDialog
import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Bundle
import android.text.InputType
import android.view.Gravity
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.graphics.drawable.GradientDrawable
import android.widget.EditText
import android.widget.FrameLayout
import android.view.inputmethod.InputMethodManager
import android.widget.LinearLayout
import android.widget.TextView

private const val WS_RETRY_MS = 800L // la lista viaja por UDP sin reintento: se pide una segunda vez si no llega
private const val IME_SETTLE_MS = 250L // el teclado anima su altura: se espera a que termine
private const val WIDE_DP = 640 // desde este ancho la barra cabe en una sola fila
private const val KEY_LEFT = "\uE001"
private const val KEY_RIGHT = "\uE002"
private const val KEY_UP = "\uE003"
private const val KEY_DOWN = "\uE004"
private const val KEY_ESC = "\uE005"
private const val KEY_TAB = "\t"
private const val CLIP_PULL = "\uE028"
private const val CLIP_BEGIN = "\uE029"
private const val CLIP_END = "\uE02A"
private const val CLIP_MAX = 60_000 // caracteres
private const val KEY_COPY = "\uE026"
private const val KEY_PASTE = "\uE027"
private const val SET_SCALE = "\uE024"
private const val MOBILE_SCALE = 200 // porcentaje: mas zoom para tocar con el dedo
private const val SCALE_SETTLE_MS = 400L // margen para que la escala llegue antes de pedir el video otra vez
private const val MOD_BASE = 0xE010 // pares abajo/arriba: Super, Ctrl, Alt, Shift
private const val ONE_ROW_DP = 900 // desde este ancho toda la barra cabe en una fila
private const val LIST_WORKSPACES = "\uE021"
private const val LEND_ID = "\uE022"
private const val LEND_WINDOW = "\uE023"
private const val ZOOM_MAX = 6f
private const val ZOOM_DP = 150f // recorrido del dedo en la franja para acercar e veces

class MainActivity : Activity() {
    private lateinit var statusDot: View
    private lateinit var statusName: TextView
    private lateinit var statusState: TextView
    private lateinit var hintTitle: TextView
    private lateinit var hintBody: TextView
    private lateinit var hint: View
    private lateinit var fieldRow: View
    private lateinit var bar: View
    private lateinit var showBar: View
    private lateinit var pad: PadView
    private lateinit var field: KeyField
    private lateinit var video: SurfaceView
    private lateinit var stage: FrameLayout
    private var screen: Screen? = null
    private var surfaceReady = false
    private var zoom = 1f
    private var vw = 0
    private var vh = 0
    private var imeH = 0
    private val resize = Runnable { if (screen != null) syncScreen() }
    private var mobile = false
    private var clipStamp = 0L // marca de tiempo del portapapeles del telefono ya sincronizado
    private val mods = ArrayList<Toggle>() // Super, Ctrl, Alt, Shift: se sueltan solos tras la siguiente tecla
    private lateinit var wifiLock: WifiManager.WifiLock
    private var pairing: Pairing? = null
    private var link: Link? = null
    private var resumed = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        // El teclado tapa la parte baja del pad en vez de reacomodarlo: su tamano fisico va en HELLO.
        // RESIZE sin ajuste del decor: nada se redimensiona, pero llegan los insets del teclado.
        window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        window.setDecorFitsSystemWindows(false)
        window.attributes = window.attributes.apply {
            layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
        }
        wifiLock = (getSystemService(Context.WIFI_SERVICE) as WifiManager)
            .createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "spenmouse:pad")

        val dp = resources.displayMetrics.density
        val target = (TARGET_DP * dp).toInt()
        val gap = (GAP_DP * dp).toInt()
        fun label(size: Float, color: Int) = TextView(this).apply {
            textSize = size
            setTextColor(color)
            maxLines = 1
            ellipsize = android.text.TextUtils.TruncateAt.END
        }

        // Estado: punto de color + nombre + texto, para que no dependa solo del color.
        statusDot = View(this)
        statusName = label(14f, C_TEXT).apply { typeface = android.graphics.Typeface.create("sans-serif-medium", android.graphics.Typeface.NORMAL) }
        statusState = label(12f, C_MUTED)
        val status = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            addView(statusDot, LinearLayout.LayoutParams(gap, gap).apply { marginEnd = gap })
            addView(LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.VERTICAL
                addView(statusName)
                addView(statusState)
            })
        }

        field = KeyField(this) { typed(it) }.apply {
            background = rounded(C_SURFACE_2)
            setTextColor(C_TEXT)
            setHintTextColor(C_MUTED)
            minHeight = target
            setPadding(2 * gap, 0, 2 * gap, 0)
        }
        // Un modificador queda sostenido hasta la siguiente tecla (o hasta volver a tocarlo): sirve con mouse.
        fun modCode(i: Int, on: Boolean) = (MOD_BASE + 2 * i + if (on) 0 else 1).toChar().toString()
        fun mod(i: Int, name: String) = TextKey(this, name, name, onChange = { on -> link?.sendText(modCode(i, on)) })
        val superButton = mod(0, "Super")
        val ctrl = mod(1, "Ctrl")
        val alt = mod(2, "Alt")
        val shift = mod(3, "Shift")
        mods += listOf(superButton, ctrl, alt, shift)
        fun arrow(name: String, key: String, turn: Float) =
            IconButton(this, R.drawable.ic_arrow, name) { typed(key) }.apply { rotation = turn }
        fun LinearLayout.add(v: View) = addView(v, LinearLayout.LayoutParams(target, target).apply { marginStart = gap })

        // Fila A: estado y modos. Fila B: teclas. Una al lado de la otra si hay ancho, apiladas si no.
        val rowA = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            addView(status, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
            add(IconToggle(this@MainActivity, R.drawable.ic_keyboard, "Teclado", ::showKeyboard))
            add(IconToggle(this@MainActivity, R.drawable.ic_screen, "Pantalla", ::showScreen))
            add(IconToggle(this@MainActivity, R.drawable.ic_touch, "Modo móvil: tocar directo y más zoom", ::setMobile))
            add(IconButton(this@MainActivity, R.drawable.ic_workspace, "Workspaces", ::askWorkspaces))
            add(IconButton(this@MainActivity, R.drawable.ic_link, "Emparejar", ::askUri))
        }
        val rowB = LinearLayout(this).apply {
            gravity = Gravity.END or Gravity.CENTER_VERTICAL
            addView(IconButton(this@MainActivity, R.drawable.ic_chevron, "Ocultar controles") { barVisible(false) }, LinearLayout.LayoutParams(target, target))
            addView(View(this@MainActivity), LinearLayout.LayoutParams(0, 0, 1f))
            add(superButton)
            add(arrow("Flecha izquierda", KEY_LEFT, 0f))
            add(arrow("Flecha abajo", KEY_DOWN, 270f))
            add(arrow("Flecha arriba", KEY_UP, 90f))
            add(arrow("Flecha derecha", KEY_RIGHT, 180f))
        }
        val rowC = LinearLayout(this).apply {
            gravity = Gravity.END or Gravity.CENTER_VERTICAL
            add(TextKey(this@MainActivity, "Esc", "Escape", onTap = { typed(KEY_ESC) }))
            add(TextKey(this@MainActivity, "Tab", "Tabulador", onTap = { typed(KEY_TAB) }))
            add(ctrl)
            add(alt)
            add(shift)
        }
        val rowD = LinearLayout(this).apply {
            gravity = Gravity.END or Gravity.CENTER_VERTICAL
            add(TextKey(this@MainActivity, "Copiar", "Copiar la selección en la laptop", onTap = { typed(KEY_COPY) }))
            add(TextKey(this@MainActivity, "Pegar", "Pegar en la laptop", onTap = { typed(KEY_PASTE) }))
        }
        // Teclas: una fila si hay ancho, una por grupo si no.
        val keys = LinearLayout(this).apply {
            gravity = Gravity.END
            addView(rowB)
            addView(rowC)
            addView(rowD)
        }
        val bar = LinearLayout(this).apply {
            setBackgroundColor(C_SURFACE)
            addView(rowA)
            addView(keys)
        }
        this.bar = bar
        // Con la barra oculta queda solo este boton flotante, en la esquina, para volver a mostrarla.
        showBar = IconButton(this, R.drawable.ic_chevron, "Mostrar controles") { barVisible(true) }.apply {
            rotation = 180f
            alpha = 0.7f
            visibility = View.GONE
        }
        // Angosto (vertical): estado / flechas / teclas. Medio: estado / flechas + teclas. Ancho: todo en una fila.
        var layoutW = -1
        bar.addOnLayoutChangeListener { _, l, _, r, _, _, _, _, _ ->
            if (r - l != layoutW) bar.post {
                layoutW = r - l
                val oneRow = layoutW >= ONE_ROW_DP * dp
                val keysInRow = layoutW >= WIDE_DP * dp
                val wrap = LinearLayout.LayoutParams.WRAP_CONTENT
                val match = LinearLayout.LayoutParams.MATCH_PARENT
                bar.orientation = if (oneRow) LinearLayout.HORIZONTAL else LinearLayout.VERTICAL
                keys.orientation = if (keysInRow) LinearLayout.HORIZONTAL else LinearLayout.VERTICAL
                rowA.layoutParams = if (oneRow) LinearLayout.LayoutParams(0, wrap, 1f) else LinearLayout.LayoutParams(match, wrap)
                keys.layoutParams = if (oneRow) LinearLayout.LayoutParams(wrap, wrap) else LinearLayout.LayoutParams(match, wrap).apply { topMargin = gap }
                rowB.layoutParams = LinearLayout.LayoutParams(if (keysInRow) wrap else match, wrap)
                rowC.layoutParams = LinearLayout.LayoutParams(if (keysInRow) wrap else match, wrap).apply { if (!keysInRow) topMargin = gap }
                rowD.layoutParams = LinearLayout.LayoutParams(if (keysInRow) wrap else match, wrap).apply { if (!keysInRow) topMargin = gap }
            }
        }

        pad = PadView(this).apply {
            contentDescription = "Área táctil"
            absolute = { mobile && video.visibility == View.VISIBLE }
            absRect = { android.graphics.Rect(video.left, video.top, video.right, video.bottom) }
            setBackgroundColor(0) // el fondo y la guia viven en el stage, debajo
            // Franjas = lo que el video sin zoom no cubre: izquierda/arriba acerca, derecha/abajo mueve.
            barAt = { x, y ->
                val fits = video.visibility != View.VISIBLE || (video.width >= stage.width && video.height >= stage.height)
                if (fits) 0 else if (x < video.left || y < video.top) 1 else if (x > video.right || y > video.bottom) 2 else 0
            }
            onBar = { mode, dx, dy ->
                if (mode == 1) zoom = (zoom * kotlin.math.exp(-dy / (ZOOM_DP * dp))).coerceIn(1f, ZOOM_MAX)
                applyZoom(if (mode == 2) dx else 0f, if (mode == 2) dy else 0f)
            }
        }
        video = SurfaceView(this).apply {
            visibility = View.GONE
            holder.addCallback(object : SurfaceHolder.Callback {
                override fun surfaceCreated(h: SurfaceHolder) {
                    surfaceReady = true
                    syncScreen()
                }

                override fun surfaceChanged(h: SurfaceHolder, format: Int, width: Int, height: Int) {}

                override fun surfaceDestroyed(h: SurfaceHolder) {
                    surfaceReady = false
                    syncScreen()
                }
            })
        }
        video = SurfaceView(this).apply {
            visibility = View.GONE
            holder.addCallback(object : SurfaceHolder.Callback {
                override fun surfaceCreated(h: SurfaceHolder) {
                    surfaceReady = true
                    syncScreen()
                }

                override fun surfaceChanged(h: SurfaceHolder, format: Int, width: Int, height: Int) {}

                override fun surfaceDestroyed(h: SurfaceHolder) {
                    surfaceReady = false
                    syncScreen()
                }
            })
        }
        // Guia del area vacia: va debajo del pad, asi no intercepta toques.
        hintTitle = label(16f, C_MUTED).apply { gravity = Gravity.CENTER }
        hintBody = label(13f, C_MUTED).apply {
            gravity = Gravity.CENTER
            maxLines = 5
            setLineSpacing(0f, 1.4f)
            setPadding(0, gap, 0, 0)
        }
        hint = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(4 * gap, 0, 4 * gap, 0)
            addView(hintTitle)
            addView(hintBody)
        }
        // El video va detras del pad: los gestos siguen llegando al PadView igual que sin video.
        stage = FrameLayout(this).apply {
            setBackgroundColor(C_BG)
            addView(hint)
            addView(video, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT, Gravity.CENTER))
            addView(pad)
            addView(showBar, FrameLayout.LayoutParams(target, target, Gravity.TOP or Gravity.END).apply { setMargins(gap, gap, gap, gap) })
        }
        // Giro (o cualquier cambio de tamano del area): se pide el video otra vez con el tamano nuevo.
        // Va por layout y no por onConfigurationChanged, que en este telefono no llego al girar.
        stage.addOnLayoutChangeListener { _, l, t, r, b, ol, ot, or, ob ->
            if (r - l != or - ol || b - t != ob - ot) stage.post {
                vw = 0
                video.layoutParams = video.layoutParams.apply {
                    width = FrameLayout.LayoutParams.MATCH_PARENT
                    height = FrameLayout.LayoutParams.MATCH_PARENT
                }
                syncScreen()
            }
        }
        fieldRow = FrameLayout(this).apply {
            setBackgroundColor(C_SURFACE)
            setPadding(gap, 0, gap, gap)
            addView(field)
        }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(bar)
            addView(fieldRow)
            addView(View(this@MainActivity).apply { setBackgroundColor(C_BORDER) }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 1))
            addView(stage, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
        })
        fieldRow.visibility = View.GONE

        // La barra respeta el recorte de la camara; el area tactil si llega a los bordes.
        bar.setOnApplyWindowInsetsListener { v, insets ->
            val c = insets.displayCutout
            v.setPadding(gap + (c?.safeInsetLeft ?: 0), gap + (c?.safeInsetTop ?: 0), gap + (c?.safeInsetRight ?: 0), gap)
            insets
        }

        // Con el teclado abierto el video se acomoda en el espacio que queda arriba.
        window.decorView.setOnApplyWindowInsetsListener { v, insets ->
            val h = insets.getInsets(WindowInsets.Type.ime()).bottom
            if (h != imeH) {
                imeH = h
                fitVideo()
                // El monitor del telefono se encoge a lo que queda visible sobre el teclado: las
                // ventanas se reacomodan ahi y se ve completo lo que se escribe, sin zoom.
                stage.removeCallbacks(resize)
                stage.postDelayed(resize, IME_SETTLE_MS)
            }
            v.onApplyWindowInsets(insets)
        }

        val p = getPreferences(MODE_PRIVATE)
        val key = p.getString("key", null)
        if (key != null) pairing = Pairing(
            p.getString("host", "")!!, p.getInt("port", DEFAULT_PORT), key, p.getString("name", "")!!,
            p.getString("alt", "")!!.split(',').filter { it.isNotEmpty() },
        )
        showStatus(false)
        handleIntent(intent)
    }

    private fun showKeyboard(on: Boolean) {
        val imm = getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        fieldRow.visibility = if (on) View.VISIBLE else View.GONE
        if (on) {
            field.requestFocus()
            field.post { imm.showSoftInput(field, 0) }
        } else {
            imm.hideSoftInputFromWindow(field.windowToken, 0)
            field.reset()
        }
    }

    /** Modo movil: toque directo sobre el video y monitor con mas zoom. Modo mouse: todo relativo. */
    private fun setMobile(on: Boolean) {
        mobile = on
        link?.sendText(SET_SCALE + (if (on) MOBILE_SCALE else 0) + ";")
        stage.removeCallbacks(resize)
        stage.postDelayed(resize, SCALE_SETTLE_MS)
    }

    // Portapapeles compartido. Android solo deja leerlo con la app al frente: al volver a ella se manda
    // lo copiado en el telefono, o si no cambio, se pide lo de la laptop.
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus) return
        val l = link ?: return
        val cm = getSystemService(android.content.ClipboardManager::class.java)
        val d = cm.primaryClipDescription // la descripcion no dispara el aviso de "pegado"
        val stamp = d?.timestamp ?: 0L
        if (d != null && stamp != clipStamp && d.hasMimeType("text/*")) {
            clipStamp = stamp
            // sin caracteres de uso privado: en el flujo de texto son ordenes para la laptop
            val text = cm.primaryClip?.getItemAt(0)?.coerceToText(this)?.toString().orEmpty().filter { it !in '\uE000'..'\uF8FF' }.take(CLIP_MAX)
            if (text.isNotEmpty()) {
                l.sendText(CLIP_BEGIN + text + CLIP_END)
                return
            }
        }
        l.sendText(CLIP_PULL)
    }

    private fun setPhoneClipboard(text: String) {
        val cm = getSystemService(android.content.ClipboardManager::class.java)
        cm.setPrimaryClip(android.content.ClipData.newPlainText("laptop", text))
        clipStamp = cm.primaryClipDescription?.timestamp ?: 0L
    }

    private fun barVisible(visible: Boolean) {
        bar.visibility = if (visible) View.VISIBLE else View.GONE
        showBar.visibility = if (visible) View.GONE else View.VISIBLE
    }

    private fun typed(s: String) {
        link?.sendText(s)
        mods.forEach { it.isOn = false }
    }

    /** Ajuste con bandas dentro del area libre del pad (lo que no tapa el teclado). */
    private fun fitVideo() {
        if (vw == 0) return
        val k = minOf(stage.width.toFloat() / vw, (stage.height - imeH).coerceAtLeast(1).toFloat() / vh)
        video.layoutParams = (video.layoutParams as FrameLayout.LayoutParams).apply {
            width = (vw * k).toInt()
            height = (vh * k).toInt()
            gravity = if (imeH > 0) Gravity.TOP or Gravity.CENTER_HORIZONTAL else Gravity.CENTER
        }
        video.post { applyZoom(0f, 0f) }
    }

    private var wsDialog: android.app.Dialog? = null
    private var wsPending = false

    private fun askWorkspaces() {
        wsPending = true
        typed(LIST_WORKSPACES)
        stage.postDelayed({ if (wsPending) typed(LIST_WORKSPACES) }, WS_RETRY_MS)
    }

    private fun showWorkspaces(list: String) {
        if (!wsPending) return // respuesta repetida del reintento
        wsPending = false
        wsDialog?.dismiss()
        wsDialog = showWorkspacePanel(
            parseWorkspaces(list), video.visibility == View.VISIBLE, stage.width,
            onWorkspace = { typed(LEND_ID + it.id + ";") },
            onWindow = { typed(LEND_WINDOW + it.addr + ";") },
        )
    }

    private fun applyZoom(dx: Float, dy: Float) {
        val mx = (zoom - 1) * video.width / 2
        val my = (zoom - 1) * video.height / 2
        video.scaleX = zoom
        video.scaleY = zoom
        video.translationX = (video.translationX + dx).coerceIn(-mx, mx)
        video.translationY = (video.translationY + dy).coerceIn(-my, my)
    }

    private fun showScreen(on: Boolean) {
        zoom = 1f
        vw = 0
        applyZoom(0f, 0f)
        hint.visibility = if (on) View.GONE else View.VISIBLE
        video.layoutParams = video.layoutParams.apply {
            width = FrameLayout.LayoutParams.MATCH_PARENT
            height = FrameLayout.LayoutParams.MATCH_PARENT
        }
        video.visibility = if (on) View.VISIBLE else View.GONE // crea o destruye la Surface -> syncScreen
    }

    /** El video corre solo con el boton activo, la Surface viva y un Link en primer plano. */
    private fun syncScreen() {
        screen?.stop()
        screen = null
        val l = link ?: return
        if (!surfaceReady) return
        if (stage.width == 0) return
        // El monitor virtual toma el tamano del area de video, en la orientacion actual.
        android.util.Log.i("spenmouse", "video: pido ${stage.width}x${stage.height - imeH}")
        screen = Screen(pairing!!, l, video.holder.surface, stage.width, (stage.height - imeH).coerceAtLeast(1)) { vw, vh ->
            runOnUiThread {
                this.vw = vw
                this.vh = vh
                fitVideo()
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    private fun handleIntent(intent: Intent) {
        if (intent.action == Intent.ACTION_VIEW) intent.dataString?.let(::pair)
        // `am start --ez workspaces true`: abre el panel sin tocar la pantalla (atajos, pruebas).
        if (intent.getBooleanExtra("workspaces", false)) stage.postDelayed(::askWorkspaces, 1500)
    }

    override fun onResume() {
        super.onResume()
        resumed = true
        window.insetsController?.let {
            it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            it.hide(WindowInsets.Type.systemBars())
        }
        wifiLock.acquire()
        startLink()
    }

    override fun onPause() {
        resumed = false
        mods.forEach { it.isOn = false }
        stopLink()
        wifiLock.release()
        super.onPause()
    }

    private fun startLink() {
        val p = pairing ?: return
        lateinit var l: Link
        val prefs = getPreferences(MODE_PRIVATE)
        l = Link(
            p,
            onWorkspaces = { list -> runOnUiThread { if (link === l) showWorkspaces(list) } },
            // MACs de la laptop vistas en cada red: con ellas la app la despierta al abrirse.
            macs = { prefs.getStringSet("macs", emptySet()).orEmpty() },
            onClip = { text -> runOnUiThread { if (link === l) setPhoneClipboard(text) } },
            onMac = { mac -> prefs.edit().putStringSet("macs", prefs.getStringSet("macs", emptySet()).orEmpty() + mac).apply() },
        ) { connected -> runOnUiThread { if (link === l) showStatus(connected) } }
        link = l
        pad.link = l
        l.start()
        syncScreen()
    }

    private fun stopLink() {
        pad.link = null
        link?.stop()
        link = null
        syncScreen()
        showStatus(false)
    }

    private fun showStatus(connected: Boolean) {
        val p = pairing
        statusName.text = p?.name ?: "Sin emparejar"
        statusState.text = when {
            p == null -> "Toca el icono de enlace"
            connected -> "Conectado"
            else -> "Sin conexión"
        }
        statusDot.background = GradientDrawable().apply {
            shape = GradientDrawable.OVAL
            setColor(if (p == null) C_MUTED else if (connected) C_OK else C_WARN)
        }
        hintTitle.text = if (p == null) "Sin emparejar" else "Touchpad"
        hintBody.text =
            if (p == null) "En la laptop ejecuta spenmouse-receiver pair\ny pega aquí la URI con el icono de enlace."
            else "Lápiz o un dedo: mover\nPunta o toque: clic  ·  Dos dedos: scroll\nBotón del lápiz: clic; sostenido, clic derecho\nModo móvil (con Pantalla): tocar directo y más zoom"
    }

    private fun pair(uri: String) {
        val p = try {
            Pairing.parse(uri)
        } catch (e: IllegalArgumentException) {
            AlertDialog.Builder(this)
                .setTitle("URI de emparejamiento inválida")
                .setMessage(e.message)
                .setPositiveButton("Aceptar", null)
                .show()
            return
        }
        getPreferences(MODE_PRIVATE).edit()
            .putString("host", p.host).putInt("port", p.port).putString("key", p.key).putString("name", p.name)
            .putString("alt", p.alt.joinToString(","))
            .putStringSet("macs", getPreferences(MODE_PRIVATE).getStringSet("macs", emptySet()).orEmpty() + p.macs)
            .apply()
        pairing = p
        clearText()
        if (resumed) {
            stopLink()
            startLink()
        } else showStatus(false)
    }

    private fun askUri() {
        val input = EditText(this).apply {
            hint = "spenmouse://pair?host=...&port=...&key=...&name=..."
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
        }
        AlertDialog.Builder(this)
            .setTitle("Pegar URI de emparejamiento")
            .setView(input)
            .setPositiveButton("Emparejar") { _, _ -> pair(input.text.toString()) }
            .setNegativeButton("Cancelar", null)
            .show()
    }
}
