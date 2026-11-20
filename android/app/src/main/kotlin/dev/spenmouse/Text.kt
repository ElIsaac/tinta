package dev.spenmouse

// Texto de la seccion 8 de CONTRACT.md. Sin imports de Android: se prueba en la JVM.

/**
 * Lo que hay que teclear para que un campo que decia [old] diga [new] con el cursor al final:
 * un U+0008 por cada code point que sobra tras el prefijo comun y luego el resto de [new].
 * Cubre agregar, borrar y las correcciones del teclado o de la escritura a mano.
 */
fun textDiff(old: String, new: String): String {
    var p = 0
    val max = minOf(old.length, new.length)
    while (p < max && old[p] == new[p]) p++
    // No partir un par sustituto: el BackSpace de la laptop borra el caracter entero.
    if (p > 0 && old[p - 1].isHighSurrogate() && (p < old.length || p < new.length)) p--
    return "\b".repeat(old.codePointCount(p, old.length)) + new.substring(p)
}

/** Saca del frente de [sb] el prefijo mas largo que quepa en [max] bytes UTF-8; null si esta vacio. */
fun takeText(sb: StringBuilder, max: Int = TEXT_MAX): ByteArray? {
    var i = 0
    var bytes = 0
    while (i < sb.length) {
        val c = sb[i]
        val pair = c.isHighSurrogate() && i + 1 < sb.length && sb[i + 1].isLowSurrogate()
        val n = if (pair) 4 else if (c.code < 0x80) 1 else if (c.code < 0x800) 2 else 3
        if (bytes + n > max) break
        bytes += n
        i += if (pair) 2 else 1
    }
    if (i == 0) return null
    val out = sb.substring(0, i).toByteArray(Charsets.UTF_8)
    sb.delete(0, i)
    return out
}
