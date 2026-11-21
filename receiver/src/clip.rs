// Shared clipboard (section 8): text only, through wl-clipboard on Linux or NSPasteboard on macOS.
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader, Write};
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
use std::sync::Mutex;

const CHUNK: usize = 1100; // bytes of text per datagram
const MAX_PARTS: usize = 32;

/// Last text synced in either direction, so a clipboard we set ourselves is not sent back.
static LAST: Mutex<String> = Mutex::new(String::new());

/// The phone sent its clipboard.
#[cfg(target_os = "linux")]
pub fn set(text: &str) {
    *LAST.lock().unwrap() = text.to_string();
    let r = Command::new("wl-copy").stdin(Stdio::piped()).spawn().and_then(|mut c| {
        c.stdin.take().unwrap().write_all(text.as_bytes())?; // dropped here = EOF
        c.wait()
    });
    if let Err(e) = r {
        eprintln!("wl-copy: {e}");
    }
}

#[cfg(target_os = "macos")]
pub fn set(text: &str) {
    *LAST.lock().unwrap() = text.to_string();
    crate::macos::clip_set(text);
}

/// Current clipboard text, or None if it is empty or not text.
#[cfg(target_os = "linux")]
pub fn get() -> Option<String> {
    let o = Command::new("wl-paste").args(["-n", "-t", "text"]).stderr(Stdio::null()).output().ok()?;
    let t = String::from_utf8_lossy(&o.stdout).into_owned();
    (o.status.success() && !t.is_empty()).then_some(t)
}

#[cfg(target_os = "macos")]
pub fn get() -> Option<String> {
    crate::macos::clip_get()
}

/// `clip \t k \t n \t chunk` texts, each small enough for one WSLIST datagram. Empty if too long.
pub fn packets(text: &str) -> Vec<String> {
    let mut chunks: Vec<&str> = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if i + c.len_utf8() - start > CHUNK {
            chunks.push(&text[start..i]);
            start = i;
        }
    }
    chunks.push(&text[start..]);
    if chunks.len() > MAX_PARTS {
        return Vec::new(); // shortcut: a clipboard over ~35 kB is not synced, raise MAX_PARTS if that hurts
    }
    let n = chunks.len();
    chunks.iter().enumerate().map(|(k, c)| format!("clip\t{}\t{n}\t{c}", k + 1)).collect()
}

/// Calls `on_change` with the new text every time the laptop's clipboard changes.
#[cfg(target_os = "linux")]
pub fn watch(on_change: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        // wl-paste runs `echo` once per change: each line is one change
        let child = Command::new("wl-paste").args(["-t", "text", "--watch", "echo"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else {
            eprintln!("clipboard: wl-paste is not available, the clipboard will not be shared");
            return;
        };
        for _ in BufReader::new(child.stdout.take().unwrap()).lines().map_while(Result::ok) {
            let Some(text) = get() else { continue };
            let mut last = LAST.lock().unwrap();
            if *last != text {
                *last = text.clone();
                drop(last);
                on_change(text);
            }
        }
    });
}

#[cfg(target_os = "macos")]
pub fn watch(on_change: impl Fn(String) + Send + 'static) {
    std::thread::spawn(move || {
        let mut last_count = crate::macos::clip_change_count();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let count = crate::macos::clip_change_count();
            if count == last_count {
                continue;
            }
            last_count = count;
            let Some(text) = get() else { continue };
            let mut last = LAST.lock().unwrap();
            if *last != text {
                *last = text.clone();
                drop(last);
                on_change(text);
            }
        }
    });
}

/// The phone asked for the clipboard: send it even if it did not change.
pub fn current() -> Option<String> {
    let text = get()?;
    *LAST.lock().unwrap() = text.clone();
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_char_boundaries() {
        assert_eq!(packets("hola"), vec!["clip\t1\t1\thola"]);
        let long = "ñ".repeat(1000); // 2000 bytes
        let p = packets(&long);
        assert_eq!(p.len(), 2);
        assert!(p[0].starts_with("clip\t1\t2\t") && p[1].starts_with("clip\t2\t2\t"));
        let joined: String = p.iter().map(|x| x.splitn(4, '\t').nth(3).unwrap()).collect();
        assert_eq!(joined, long);
        assert!(packets(&"x".repeat(CHUNK * (MAX_PARTS + 1))).is_empty());
    }
}
