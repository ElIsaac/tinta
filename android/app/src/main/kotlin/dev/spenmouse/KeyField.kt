package dev.spenmouse

import android.content.Context
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.KeyEvent
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputConnectionWrapper
import android.view.inputmethod.InputMethodManager
import android.widget.EditText

/**
 * Campo del modo teclado: lo que el teclado, el dictado o la escritura a mano del sistema dejan
 * aqui se convierte en el flujo de la seccion 8 (texto, U+0008, U+000A) y se pasa a [send].
 */
class KeyField(context: Context, private val send: (String) -> Unit) : EditText(context) {
    private var prev = ""

    init {
        hint = "Escribe aquí"
        textSize = 14f
        inputType = InputType.TYPE_CLASS_TEXT
        // Sin el editor a pantalla completa del modo horizontal: taparia el pad y el video.
        imeOptions = EditorInfo.IME_ACTION_NONE or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN
        addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence, start: Int, before: Int, count: Int) {}
            override fun afterTextChanged(s: Editable) {
                val now = s.toString()
                val d = textDiff(prev, now)
                prev = now
                if (d.isNotEmpty()) send(d)
            }
        })
        // Enter llega como accion del teclado (event == null) o como tecla (DOWN y luego UP).
        setOnEditorActionListener { _, _, e ->
            if (e == null || e.action == KeyEvent.ACTION_DOWN) {
                send("\n")
                reset() // tras Enter el teclado ya no corrige lo anterior: el campo no crece sin fin
            }
            true
        }
    }

    fun reset() {
        prev = ""
        text.clear()
        (context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager).restartInput(this)
    }

    // Con el campo vacio no hay nada que borrar aqui, pero si en la laptop.
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (keyCode == KeyEvent.KEYCODE_DEL && text.isEmpty()) {
            send("\b")
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection? {
        val ic = super.onCreateInputConnection(outAttrs) ?: return null
        return object : InputConnectionWrapper(ic, true) {
            override fun deleteSurroundingText(before: Int, after: Int): Boolean {
                if (before > 0 && text.isEmpty()) {
                    send("\b")
                    return true
                }
                return super.deleteSurroundingText(before, after)
            }

            override fun deleteSurroundingTextInCodePoints(before: Int, after: Int): Boolean {
                if (before > 0 && text.isEmpty()) {
                    send("\b")
                    return true
                }
                return super.deleteSurroundingTextInCodePoints(before, after)
            }
        }
    }
}
