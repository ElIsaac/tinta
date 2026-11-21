#[cfg(target_os = "linux")]
mod imp {
    // Lend the active workspace to the phone's monitor; it goes home on the next workspace change.
    use crate::stream::{hyprctl, virtual_exists, VIRT};
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    static LENT: Mutex<Option<(i64, String)>> = Mutex::new(None); // (workspace id, monitor it came from)
    static LENT_WINS: Mutex<Vec<(String, i64)>> = Mutex::new(Vec::new()); // (window address, workspace it came from)

    fn eval(code: &str) {
        let _ = hyprctl(&["eval", code]);
    }

    fn mv(ws: i64, mon: &str) {
        // 0.57 reads the workspace from `id`, the wiki says `workspace`: send both
        eval(&format!("hl.dispatch(hl.dsp.workspace.move({{workspace={ws}, id={ws}, monitor=\"{mon}\"}}))"));
    }

    fn focus(ws: i64) {
        eval(&format!("hl.dispatch(hl.dsp.focus({{workspace={ws}}}))"));
    }

    /// "workspace 11 (11) on monitor HDMI-A-1:" -> (11, "HDMI-A-1")
    fn parse_ws(line: &str) -> Option<(i64, String)> {
        let r = line.strip_prefix("workspace ")?;
        let r = r.strip_prefix("ID ").unwrap_or(r);
        let id = r.split(' ').next()?.parse().ok()?;
        let mon = r.rsplit_once("on monitor ")?.1.trim_end().trim_end_matches(':');
        Some((id, mon.to_string()))
    }

    fn active() -> Option<(i64, String)> {
        parse_ws(hyprctl(&["activeworkspace"]).ok()?.lines().next()?)
    }

    /// The workspace a monitor shows, from the text of `hyprctl monitors`.
    fn shown(monitors: &str, mon: &str) -> Option<i64> {
        let mut on = false;
        for l in monitors.lines() {
            if let Some(r) = l.strip_prefix("Monitor ") {
                on = r.split(' ').next() == Some(mon);
            } else if let Some(r) = l.trim().strip_prefix("active workspace: ").filter(|_| on) {
                return r.split(' ').next()?.parse().ok();
            }
        }
        None
    }

    fn workspaces(text: &str) -> Vec<(i64, String)> {
        let mut v: Vec<_> = text.lines().filter_map(parse_ws).filter(|w| w.0 > 0).collect();
        v.sort();
        v
    }

    const LIST_MAX: usize = 1350; // one UDP datagram under a 1500 MTU
    const TITLE_MAX: usize = 28;

    /// Logical rectangle of each monitor, from `hyprctl monitors`: name -> "x,y,w,h".
    fn monitor_rects(monitors_txt: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, (i32, i32, f32, f32))> = Vec::new();
        for l in monitors_txt.lines() {
            if let Some(r) = l.strip_prefix("Monitor ") {
                out.push((r.split(' ').next().unwrap_or("").to_string(), (0, 0, 0.0, 0.0)));
            } else if let Some((mode, at)) = l.trim().split_once(" at ").filter(|(m, _)| m.contains('@')) {
                // "1920x1200@60.00300 at 0x0"
                let size = mode.split('@').next().unwrap_or("");
                let (Some((w, h)), Some((x, y)), Some(last)) = (size.split_once('x'), at.split_once('x'), out.last_mut()) else { continue };
                last.1 = (x.parse().unwrap_or(0), y.parse().unwrap_or(0), w.parse().unwrap_or(0.0), h.parse().unwrap_or(0.0));
            } else if let Some(k) = l.trim().strip_prefix("scale: ") {
                // window coordinates are logical: pixels / scale
                let (Some(last), Ok(k)) = (out.last_mut(), k.parse::<f32>()) else { continue };
                if k > 0.0 {
                    last.1 .2 /= k;
                    last.1 .3 /= k;
                }
            }
        }
        out.into_iter().map(|(n, (x, y, w, h))| (n, format!("{x},{y},{},{}", w.round() as i32, h.round() as i32))).collect()
    }

    /// One line per workspace for the phone:
    /// `id \t monitor \t mx,my,mw,mh \t addr,x,y,w,h,class: title \t ...` (logical layout coordinates).
    /// A `*` before the address marks a window lent to the phone on its own.
    // shortcut: workspaces that do not fit in one datagram are left out; split into several if that ever happens.
    fn format_list(workspaces_txt: &str, clients_txt: &str, monitors_txt: &str, lent: &[String]) -> String {
        let rects = monitor_rects(monitors_txt);
        let mut wins: Vec<(i64, String)> = Vec::new();
        let (mut addr, mut ws, mut at, mut size, mut class, mut title) = ("", 0i64, "0,0", "0,0", "", "");
        let flush = |addr: &str, ws: i64, at: &str, size: &str, class: &str, title: &str, wins: &mut Vec<(i64, String)>| {
            if ws > 0 {
                let t: String = title.chars().take(TITLE_MAX).collect();
                let mark = if lent.iter().any(|l| l == addr) { "*" } else { "" };
                wins.push((ws, format!("{mark}{addr},{at},{size},{class}: {t}").replace(['\t', '\n'], " ")));
            }
        };
        for l in clients_txt.lines() {
            if let Some(r) = l.strip_prefix("Window ") {
                flush(addr, ws, at, size, class, title, &mut wins);
                (addr, ws, at, size, class, title) = (r.split(' ').next().unwrap_or(""), 0, "0,0", "0,0", "", "");
            } else if let Some(r) = l.strip_prefix("\tworkspace: ") {
                ws = r.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            } else if let Some(r) = l.strip_prefix("\tat: ") {
                at = r;
            } else if let Some(r) = l.strip_prefix("\tsize: ") {
                size = r;
            } else if let Some(r) = l.strip_prefix("\tclass: ") {
                class = r;
            } else if let Some(r) = l.strip_prefix("\ttitle: ") {
                title = r;
            }
        }
        flush(addr, ws, at, size, class, title, &mut wins);
        let mut out = String::new();
        for (id, mon) in workspaces(workspaces_txt) {
            let rect = rects.iter().find(|r| r.0 == mon).map_or("0,0,0,0", |r| r.1.as_str());
            let mut line = format!("{id}\t{mon}\t{rect}");
            for (_, w) in wins.iter().filter(|w| w.0 == id) {
                line.push('\t');
                line.push_str(w);
            }
            line.push('\n');
            if out.len() + line.len() > LIST_MAX {
                break;
            }
            out.push_str(&line);
        }
        out
    }

    pub fn list() -> String {
        let get = |what| hyprctl(&[what]).unwrap_or_default();
        let lent: Vec<String> = LENT_WINS.lock().unwrap().iter().map(|w| w.0.clone()).collect();
        format_list(&get("workspaces"), &get("clients"), &get("monitors"), &lent)
    }

    /// Workspace a window is on, from the text of `hyprctl clients`.
    fn window_ws(clients_txt: &str, addr: &str) -> Option<i64> {
        let mut on = false;
        for l in clients_txt.lines() {
            if let Some(r) = l.strip_prefix("Window ") {
                on = r.split(' ').next() == Some(addr);
            } else if let Some(r) = l.strip_prefix("\tworkspace: ").filter(|_| on) {
                return r.split(' ').next()?.parse().ok();
            }
        }
        None
    }

    fn mv_window(addr: &str, ws: i64) {
        eval(&format!("hl.dispatch(hl.dsp.window.move({{workspace={ws}, window=\"address:0x{addr}\", follow=false}}))"));
    }

    /// Bring one window to the workspace the phone shows; a window already lent goes back instead.
    pub fn lend_window(addr: &str) {
        if addr.is_empty() || !addr.bytes().all(|b| b.is_ascii_hexdigit()) {
            return; // it goes into a Lua string
        }
        let mut lent = LENT_WINS.lock().unwrap();
        if let Some(i) = lent.iter().position(|w| w.0 == addr) {
            let (_, origin) = lent.remove(i);
            mv_window(addr, origin);
            return;
        }
        let Some(origin) = window_ws(&hyprctl(&["clients"]).unwrap_or_default(), addr) else { return };
        let Some(phone) = hyprctl(&["monitors"]).ok().and_then(|o| shown(&o, VIRT)) else { return };
        if origin == phone {
            return;
        }
        mv_window(addr, phone);
        lent.push((addr.to_string(), origin));
    }

    /// Every window lent on its own goes back to its workspace (the phone's monitor is going away).
    pub fn return_windows() {
        for (addr, origin) in LENT_WINS.lock().unwrap().drain(..) {
            mv_window(&addr, origin);
        }
    }

    /// Lend a workspace to the phone's monitor (None = the active one). Lending the one that is
    /// already lent sends it home; any other lent workspace goes home first.
    pub fn lend(id: Option<i64>) {
        let Some(id) = id.or_else(|| active().map(|a| a.0)) else { return };
        let all = workspaces(&hyprctl(&["workspaces"]).unwrap_or_default());
        let Some((_, mon)) = all.into_iter().find(|w| w.0 == id) else { return };
        let mut lent = LENT.lock().unwrap();
        if let Some((l, origin)) = lent.take() {
            mv(l, &origin);
            if l == id {
                return;
            }
        }
        if mon == VIRT || !virtual_exists() {
            return; // already there on its own, or no phone monitor to lend to
        }
        mv(id, VIRT);
        focus(id); // a workspace that was not visible does not show up on its own
        *lent = Some((id, mon));
    }

    /// Send the lent workspace home. `force` = even while it is the active one (the monitor is going away).
    pub fn give_back(force: bool) {
        let mut lent = LENT.lock().unwrap();
        let Some((ws, origin)) = lent.clone() else { return };
        let now = active();
        if !force && now.as_ref().is_some_and(|a| a.0 == ws) {
            return;
        }
        *lent = None;
        let before = hyprctl(&["monitors"]).ok().and_then(|o| shown(&o, &origin));
        mv(ws, &origin);
        if force {
            return;
        }
        // the returned workspace takes over its monitor and the focus: put both back
        if let Some(b) = before.filter(|b| *b != ws) {
            focus(b);
        }
        if let Some((n, _)) = now {
            focus(n);
        }
    }

    /// `workspace>>` is only emitted when the user switches workspace, not on mouse movement
    /// between monitors, so the lent workspace stays on the phone while the cursor wanders.
    pub fn watch() {
        let (Ok(rt), Ok(sig)) = (std::env::var("XDG_RUNTIME_DIR"), std::env::var("HYPRLAND_INSTANCE_SIGNATURE")) else {
            eprintln!("workspace: no Hyprland session in the environment, lent workspaces will not return on their own");
            return;
        };
        std::thread::spawn(move || {
            let Ok(s) = UnixStream::connect(format!("{rt}/hypr/{sig}/.socket2.sock")) else {
                eprintln!("workspace: cannot open Hyprland's event socket");
                return;
            };
            for l in BufReader::new(s).lines().map_while(Result::ok) {
                if l.starts_with("workspace>>") {
                    give_back(false);
                }
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_hyprctl_text() {
            // real 0.57 output has no "ID"; older builds printed it
            assert_eq!(parse_ws("workspace 11 (11) on monitor HDMI-A-1:"), Some((11, "HDMI-A-1".into())));
            assert_eq!(parse_ws("workspace ID 11 (11) on monitor HDMI-A-1:"), Some((11, "HDMI-A-1".into())));
            assert_eq!(parse_ws("nope"), None);
            let m = "Monitor eDP-1 (ID 0):\n\t1920x1200@60 at 0x0\n\tactive workspace: 1 (1)\n\nMonitor HDMI-A-1 (ID 1):\n\tactive workspace: 11 (11)\n";
            assert_eq!(shown(m, "HDMI-A-1"), Some(11));
            assert_eq!(shown(m, "eDP-1"), Some(1));
            assert_eq!(shown(m, "DP-9"), None);
        }

        #[test]
        fn lists_workspaces_with_their_windows() {
            let ws = "workspace 2 (2) on monitor eDP-1:\n\tmonitorID: 0\n\nworkspace 11 (11) on monitor spenmouse:\n\nworkspace -98 (special) on monitor eDP-1:\n";
            let cl = "Window 55a -> vpn 1:\n\tmapped: 1\n\tat: 12,48\n\tsize: 343,563\n\tworkspace: 2 (2)\n\tclass: kitty\n\ttitle: vpn 1\n\nWindow 55b -> x:\n\tat: 3852,48\n\tsize: 700,500\n\tworkspace: 11 (11)\n\tclass: firefox\n\ttitle: Docs, v2\n";
            let mo = "Monitor eDP-1 (ID 0):\n\t1920x1200@60.00300 at 0x0\n\tscale: 1\n\nMonitor spenmouse (ID 2):\n\t2340x942@60.00000 at 3840x0\n\tscale: 1.50\n";
            assert_eq!(
                format_list(ws, cl, mo, &["55b".to_string()]),
                "2\teDP-1\t0,0,1920,1200\t55a,12,48,343,563,kitty: vpn 1\n11\tspenmouse\t3840,0,1560,628\t*55b,3852,48,700,500,firefox: Docs, v2\n"
            );
            assert_eq!(window_ws(cl, "55b"), Some(11));
            assert_eq!(window_ws(cl, "nope"), None);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::macos;
    use std::sync::Mutex;

    type SavedWin = (String, f64, f64, f64, f64);
    static LENT: Mutex<Option<(i64, Vec<SavedWin>)>> = Mutex::new(None);
    static LENT_WINS: Mutex<Vec<SavedWin>> = Mutex::new(Vec::new());

    fn parse_addr(addr: &str) -> Option<(i32, i32)> {
        let (p, w) = addr.split_once(':')?;
        let pid: i32 = p.parse().ok()?;
        let wid: i32 = w.parse().ok()?;
        (pid > 0 && wid > 0).then_some((pid, wid))
    }

    fn windows_on_display_filtered(
        list_txt: &str,
        display_id: i64,
        include_lent: bool,
    ) -> Vec<(String, f64, f64)> {
        let prefix = format!("{display_id}\t");
        let Some(line) = list_txt.lines().find(|l| l.starts_with(&prefix)) else {
            return Vec::new();
        };
        line.split('\t')
            .skip(3)
            .filter_map(|w| {
                let mut parts = w.splitn(6, ',');
                let raw_addr = parts.next()?;
                let addr = if let Some(stripped) = raw_addr.strip_prefix('*') {
                    if !include_lent {
                        return None;
                    }
                    stripped
                } else {
                    raw_addr
                };
                let x: f64 = parts.next()?.parse().ok()?;
                let y: f64 = parts.next()?.parse().ok()?;
                Some((addr.to_string(), x, y))
            })
            .collect()
    }

    fn windows_on_display(list_txt: &str, display_id: i64) -> Vec<(String, f64, f64)> {
        windows_on_display_filtered(list_txt, display_id, false)
    }

    pub fn refit_virtual() {
        let virt = macos::virtual_id();
        if virt == 0 {
            return;
        }
        let Some((vx, vy, vw, vh)) = macos::display_bounds(virt) else { return };
        let (tx, ty, tw, th) = fit_rect(vx, vy, vw, vh);
        for (addr, _, _) in windows_on_display_filtered(&macos::ws_list(""), virt as i64, true)
            .iter()
            .rev()
        {
            if let Some((pid, wid)) = parse_addr(addr) {
                macos::move_window(pid, wid, tx, ty, tw, th, false);
            }
        }
    }

    fn fit_rect(vx: f64, vy: f64, vw: f64, vh: f64) -> (f64, f64, f64, f64) {
        (
            vx + 8.0,
            vy + 32.0,
            (vw - 16.0).max(320.0),
            (vh - 40.0).max(240.0),
        )
    }

    fn restore_saved(saved: Vec<SavedWin>, raise: bool) {
        for (addr, ox, oy, ow, oh) in saved {
            if let Some((pid, wid)) = parse_addr(&addr) {
                macos::move_window(pid, wid, ox, oy, ow, oh, raise);
            }
        }
    }

    pub fn list() -> String {
        let csv: String = LENT_WINS
            .lock()
            .unwrap()
            .iter()
            .map(|w| w.0.as_str())
            .collect::<Vec<_>>()
            .join(",");
        macos::ws_list(&csv)
    }

    pub fn lend_window(addr: &str) {
        let Some((pid, wid)) = parse_addr(addr) else { return };
        if crate::stream::is_window_mode() {
            crate::stream::select_window(pid, wid as u32);
            macos::focus_window(pid, wid);
            return;
        }
        {
            let mut lent = LENT_WINS.lock().unwrap();
            if let Some(i) = lent.iter().position(|w| w.0 == addr) {
                let (_, ox, oy, ow, oh) = lent.remove(i);
                drop(lent);
                macos::move_window(pid, wid, ox, oy, ow, oh, true);
                return;
            }
        }
        let virt = macos::virtual_id();
        let target = if virt != 0 { virt } else { macos::target_display() };
        if windows_on_display(&macos::ws_list(""), target as i64)
            .iter()
            .any(|(a, _, _)| a == addr)
        {
            macos::focus_window(pid, wid);
            return;
        }
        let Some((vx, vy, vw, vh)) = macos::display_bounds(target) else { return };
        let (tx, ty, tw, th) = fit_rect(vx, vy, vw, vh);
        if let Some((ox, oy, ow, oh)) = macos::move_window(pid, wid, tx, ty, tw, th, true) {
            if virt != 0 && target == virt {
                let mut lent = LENT_WINS.lock().unwrap();
                if !lent.iter().any(|w| w.0 == addr) {
                    lent.push((addr.to_string(), ox, oy, ow, oh));
                }
            }
        }
    }

    pub fn return_windows() {
        let saved: Vec<SavedWin> = LENT_WINS.lock().unwrap().drain(..).collect();
        restore_saved(saved, false);
    }

    pub fn lend(id: Option<i64>) {
        let target_id = id.unwrap_or_else(|| macos::focused_display() as i64);
        if crate::stream::is_window_mode() {
            let picked = windows_on_display(&list(), target_id)
                .into_iter()
                .find_map(|(addr, _, _)| parse_addr(&addr).map(|(p, w)| (p, w as u32)))
                .or_else(macos::frontmost_window);
            if let Some((pid, wid)) = picked {
                crate::stream::select_window(pid, wid);
                macos::focus_window(pid, wid as i32);
            }
            return;
        }
        let mut lent = LENT.lock().unwrap();
        let prev_id = lent.take().map(|(prev, saved)| {
            restore_saved(saved, false);
            prev
        });
        let virt = macos::virtual_id();
        if virt != 0 && target_id == virt as i64 {
            drop(lent);
            return_windows();
            return;
        }
        if prev_id == Some(target_id) {
            return;
        }
        let dest = if virt != 0 { virt } else { macos::target_display() };
        if target_id == dest as i64 {
            for (addr, _, _) in windows_on_display(&macos::ws_list(""), dest as i64) {
                if let Some((pid, wid)) = parse_addr(&addr) {
                    if macos::focus_window(pid, wid) {
                        break;
                    }
                }
            }
            return;
        }
        let Some((vx, vy, vw, vh)) = macos::display_bounds(dest) else { return };
        let (base_x, base_y, base_w, base_h) = fit_rect(vx, vy, vw, vh);
        let max_offset = (base_w - 320.0).max(0.0).min((base_h - 240.0).max(0.0));
        let wins = windows_on_display(&list(), target_id);
        let mut moved = Vec::new();
        // CGWindowList returns windows front-to-back, so move back-to-front and raise index 0 last.
        for (i, (addr, _, _)) in wins.iter().enumerate().rev() {
            let Some((pid, wid)) = parse_addr(addr) else { continue };
            let offset = if i > 0 {
                ((i as f64) * 16.0).min(max_offset)
            } else {
                0.0
            };
            let tx = base_x + offset;
            let ty = base_y + offset;
            let tw = (base_w - offset).max(320.0);
            let th = (base_h - offset).max(240.0);
            if let Some((ox, oy, ow, oh)) = macos::move_window(pid, wid, tx, ty, tw, th, i == 0) {
                moved.push((addr.clone(), ox, oy, ow, oh));
            }
        }
        if virt != 0 && dest == virt && !moved.is_empty() {
            *lent = Some((target_id, moved));
        }
    }

    pub fn give_back(force: bool) {
        if !force {
            return;
        }
        let saved = {
            let mut lent = LENT.lock().unwrap();
            let Some((_, saved)) = lent.take() else { return };
            saved
        };
        restore_saved(saved, false);
    }

    pub fn watch() {}

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_macos_window_addrs_and_list_lines() {
            assert_eq!(parse_addr("123:456"), Some((123, 456)));
            assert_eq!(parse_addr("0:456"), None);
            assert_eq!(parse_addr("bad"), None);

            let txt = "1\tBuilt-in\t0,0,1512,982\t100:1,10,20,800,600,Safari: Docs\t*100:2,30,40,500,400,Notes: Note\n";
            assert_eq!(
                windows_on_display(txt, 1),
                vec![("100:1".to_string(), 10.0, 20.0)]
            );
            assert_eq!(
                windows_on_display_filtered(txt, 1, true),
                vec![
                    ("100:1".to_string(), 10.0, 20.0),
                    ("100:2".to_string(), 30.0, 40.0),
                ]
            );
            assert!(windows_on_display(txt, 2).is_empty());
        }

        #[test]
        fn fits_lent_window_rect_with_clamping() {
            assert_eq!(
                fit_rect(1512.0, 0.0, 1560.0, 628.0),
                (1520.0, 32.0, 1544.0, 588.0)
            );
            assert_eq!(
                fit_rect(100.0, 50.0, 300.0, 200.0),
                (108.0, 82.0, 320.0, 240.0)
            );
        }
    }
}

pub use imp::{give_back, lend, lend_window, list, return_windows, watch};
#[cfg(target_os = "macos")]
pub use imp::refit_virtual;
