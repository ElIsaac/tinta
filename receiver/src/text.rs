// Section 8: type received text into the focused window with wtype(1).
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, PartialEq)]
pub enum Seg<'a> {
    Text(&'a str),
    Key(&'static str),
    Lend(Option<i64>), // lend this workspace to the phone (None = the active one)
    Window(&'a str), // lend this window (address in hex) to the phone, or send it back
    Mod(usize, bool), // index into MODS, down/up
    Scale(u32), // scale of the phone's monitor, in percent
    Clip(bool), // copy (false) or paste (true) with the shortcut the focused app uses
}

/// U+0008 / U+000A / U+0009 and the U+E0xx codes of section 8 become keys, in order with the text.
pub fn segments(s: &str) -> Vec<Seg<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut skip = 0; // end of the digits of a U+E022 already consumed
    for (i, c) in s.char_indices() {
        if i < skip {
            continue;
        }
        let mut end = i + c.len_utf8();
        let seg = match c {
            '\u{8}' => Some(Seg::Key("BackSpace")),
            '\n' => Some(Seg::Key("Return")),
            '\t' => Some(Seg::Key("Tab")),
            '\u{e001}' => Some(Seg::Key("Left")),
            '\u{e002}' => Some(Seg::Key("Right")),
            '\u{e003}' => Some(Seg::Key("Up")),
            '\u{e004}' => Some(Seg::Key("Down")),
            '\u{e005}' => Some(Seg::Key("Escape")),
            '\u{e026}' => Some(Seg::Clip(false)),
            '\u{e027}' => Some(Seg::Clip(true)),
            // U+E010..U+E017: down/up pairs for Super, Ctrl, Alt, Shift
            '\u{e010}'..='\u{e017}' => {
                let n = c as usize - 0xe010;
                Some(Seg::Mod(n / 2, n % 2 == 0))
            }
            '\u{e020}' => Some(Seg::Lend(None)),
            '\u{e021}' | '\u{e025}' | '\u{e028}' => None, // list / MAC / clipboard request: answered by the UDP loop
            '\u{e022}'..='\u{e024}' => {
                // U+E022 <workspace id> ';', U+E023 <window address> ';', U+E024 <scale percent> ';'
                let stop = s[end..].find(';').map_or(s.len(), |n| end + n);
                let arg = &s[end..stop];
                end = (stop + 1).min(s.len());
                skip = end;
                match c {
                    '\u{e022}' => arg.parse().ok().map(|id| Seg::Lend(Some(id))),
                    '\u{e023}' => Some(Seg::Window(arg)),
                    _ => arg.parse().ok().map(Seg::Scale),
                }
            }
            _ => continue,
        };
        if start < i {
            out.push(Seg::Text(&s[start..i]));
        }
        out.extend(seg);
        start = end;
    }
    if start < s.len() {
        out.push(Seg::Text(&s[start..]));
    }
    out
}

/// --dry-run form: `"Hola" Return BackSpace`.
pub fn describe(s: &str) -> String {
    let parts: Vec<String> = segments(s)
        .iter()
        .map(|g| match g {
            Seg::Text(t) => format!("{t:?}"),
            Seg::Key(k) => k.to_string(),
            Seg::Lend(None) => "Workspace".to_string(),
            Seg::Lend(Some(id)) => format!("Workspace({id})"),
            Seg::Window(a) => format!("Window({a})"),
            Seg::Scale(p) => format!("Scale({p})"),
            Seg::Clip(paste) => if *paste { "Paste" } else { "Copy" }.to_string(),
            Seg::Mod(i, d) => format!("{}{}", MODS[*i].0, if *d { '+' } else { '-' }),
        })
        .collect();
    parts.join(" ")
}

fn wtype(args: &[&str], stdin: Option<&str>) {
    let r = Command::new("wtype")
        .args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .spawn()
        .and_then(|mut c| {
            if let Some(t) = stdin {
                c.stdin.take().unwrap().write_all(t.as_bytes())?; // dropped here = EOF
            }
            c.wait()
        });
    match r {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("wtype: {s}"),
        Err(e) => eprintln!("wtype: {e}"),
    }
}

const CLIP_MAX: usize = 256 * 1024;
const TERMINALS: [&str; 9] = ["kitty", "foot", "alacritty", "wezterm", "ghostty", "konsole", "terminal", "xterm", "tilix"];

fn is_terminal(activewindow_txt: &str) -> bool {
    let class = activewindow_txt.lines().find_map(|l| l.trim().strip_prefix("class: ")).unwrap_or("").to_lowercase();
    TERMINALS.iter().any(|t| class.contains(t))
}

fn focused_is_terminal() -> bool {
    crate::stream::hyprctl(&["activewindow"]).is_ok_and(|o| is_terminal(&o))
}

/// Modifier name for wtype and its evdev key code, indexed like the U+E010.. pairs of section 8.
pub const MODS: [(&str, u16); 4] = [("logo", 125), ("ctrl", 29), ("alt", 56), ("shift", 42)];

/// The typing thread's state: which modifiers the phone is holding.
/// Each one is held twice on purpose: as a real key through uinput, so it works with the mouse
/// (Super+drag, Ctrl+click) and with Hyprland binds, and as `-M` on every wtype call, because
/// an app only sees the modifiers of the keyboard that sent the key.
pub struct Typist {
    kbd: Option<std::fs::File>, // None in --dry-run
    held: [bool; 4],
    clip: Option<String>, // clipboard text being received (it can span several TEXT packets)
}

impl Typist {
    pub fn new(kbd: Option<std::fs::File>) -> Typist {
        Typist { kbd, held: [false; 4], clip: None }
    }

    pub fn any_held(&self) -> bool {
        self.held.contains(&true)
    }

    pub fn release_all(&mut self) {
        for i in 0..MODS.len() {
            if self.held[i] {
                self.hold(i, false);
            }
        }
    }

    fn hold(&mut self, i: usize, down: bool) {
        self.held[i] = down;
        if let Some(k) = &self.kbd {
            crate::uinput::hold_key(k, MODS[i].1, down);
        }
    }

    fn wtype(&self, args: &[&str], stdin: Option<&str>) {
        let mut all: Vec<&str> = Vec::new();
        for (i, m) in MODS.iter().enumerate() {
            if self.held[i] {
                all.extend(["-M", m.0]);
            }
        }
        all.extend(args);
        wtype(&all, stdin);
    }

    /// Blocks until everything is typed; call it from the typing thread only.
    /// Between U+E029 and U+E02A the text is the phone's clipboard: it is stored, never typed.
    pub fn type_text(&mut self, mut s: &str) {
        loop {
            if let Some(buf) = self.clip.as_mut() {
                let (data, rest) = s.split_once('\u{e02a}').map_or((s, None), |(d, r)| (d, Some(r)));
                if buf.len() + data.len() <= CLIP_MAX {
                    buf.push_str(data);
                }
                let Some(rest) = rest else { return };
                crate::clip::set(&self.clip.take().unwrap());
                s = rest;
            }
            match s.split_once('\u{e029}') {
                Some((before, rest)) => {
                    self.type_plain(before);
                    self.clip = Some(String::new());
                    s = rest;
                }
                None => return self.type_plain(s),
            }
        }
    }

    /// Text goes through stdin so it can never be parsed as a wtype option.
    fn type_plain(&mut self, s: &str) {
        let mut keys: Vec<&str> = Vec::new(); // consecutive keys share one wtype process
        for g in segments(s) {
            if !matches!(g, Seg::Key(_)) && !keys.is_empty() {
                self.wtype(&keys, None);
                keys.clear();
            }
            match g {
                Seg::Key(k) => keys.extend(["-k", k]),
                Seg::Mod(i, d) => self.hold(i, d),
                Seg::Lend(id) => crate::ws::lend(id),
                Seg::Window(a) => crate::ws::lend_window(a),
                Seg::Scale(p) => crate::stream::set_scale(p),
                Seg::Clip(paste) => {
                    // terminals keep Ctrl+C/V for themselves and copy/paste with Shift added
                    let key = if paste { "v" } else { "c" };
                    if focused_is_terminal() {
                        self.wtype(&["-M", "ctrl", "-M", "shift", "-k", key], None);
                    } else {
                        self.wtype(&["-M", "ctrl", "-k", key], None);
                    }
                }
                Seg::Text(t) => self.wtype(&["-"], Some(t)),
            }
        }
        if !keys.is_empty() {
            self.wtype(&keys, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_stay_in_order_with_text() {
        use Seg::*;
        assert_eq!(segments("Hola \u{f1}\n\u{8}"), vec![Text("Hola \u{f1}"), Key("Return"), Key("BackSpace")]);
        assert_eq!(segments("\ta\n\nb"), vec![Key("Tab"), Text("a"), Key("Return"), Key("Return"), Text("b")]);
        assert_eq!(segments("-k Return"), vec![Text("-k Return")]);
        assert_eq!(segments(""), vec![]);
        assert_eq!(segments("\u{e010}2\u{e003}\u{e011}"), vec![Mod(0, true), Text("2"), Key("Up"), Mod(0, false)]);
        assert_eq!(segments("\u{e012}c\u{e013}\u{e005}\u{e016}\u{e017}"), vec![Mod(1, true), Text("c"), Mod(1, false), Key("Escape"), Mod(3, true), Mod(3, false)]);
        assert_eq!(segments("a\u{e021}\u{e022}11;b\u{e020}\u{e022}x;"), vec![Text("a"), Lend(Some(11)), Text("b"), Lend(None)]);
        assert_eq!(segments("\u{e023}55a1;x"), vec![Window("55a1"), Text("x")]);
        assert_eq!(segments("\u{e026}\u{e027}"), vec![Clip(false), Clip(true)]);
        assert!(is_terminal("Window 55a -> x:\n\tclass: kitty\n\ttitle: ~\n"));
        assert!(!is_terminal("Window 55a -> x:\n\tclass: google-chrome\n"));
        assert_eq!(describe("-x \"q\"\n\u{8}fin"), r#""-x \"q\"" Return BackSpace "fin""#);
    }
}
