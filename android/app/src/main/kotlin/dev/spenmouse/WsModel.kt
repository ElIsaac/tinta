package dev.spenmouse

// Lista de workspaces (WSLIST, seccion 8 de CONTRACT.md). Sin imports de Android: se prueba en la JVM.

/** [lent] = prestada al telefono por separado (tocarla la devuelve a su workspace). */
class WsWindow(val addr: String, val lent: Boolean, val x: Int, val y: Int, val w: Int, val h: Int, val label: String) {
    /** "kitty: vpn 1" -> "kitty" */
    val app get() = label.substringBefore(':')
    val title get() = label.substringAfter(": ", "")
}

class Workspace(val id: String, val monitor: String, val mx: Int, val my: Int, val mw: Int, val mh: Int, val windows: List<WsWindow>)

/** Una linea por workspace: `id TAB monitor TAB mx,my,mw,mh TAB [*]dir,x,y,w,h,clase: titulo TAB ...`. Lo que no cuadra se omite. */
fun parseWorkspaces(list: String): List<Workspace> = list.lines().mapNotNull { line ->
    val f = line.split('\t')
    if (f.size < 3 || f[0].isBlank()) return@mapNotNull null
    val m = f[2].split(',').mapNotNull { it.toIntOrNull() }
    if (m.size != 4) return@mapNotNull null
    val wins = f.drop(3).mapNotNull { w ->
        val p = w.split(',', limit = 6) // el titulo puede traer comas
        val n = p.drop(1).take(4).mapNotNull { it.toIntOrNull() }
        if (p.size == 6 && n.size == 4) WsWindow(p[0].removePrefix("*"), p[0].startsWith("*"), n[0], n[1], n[2], n[3], p[5]) else null
    }
    Workspace(f[0], f[1], m[0], m[1], m[2], m[3], wins)
}
