// What the receiver is doing, for the desktop manager and its waybar icon:
// $XDG_RUNTIME_DIR/spenmouse/status, `key=value` lines, rewritten only when something changes.
use std::sync::Mutex;

static STATE: Mutex<(String, String)> = Mutex::new((String::new(), String::new())); // (phone ip, streamed output)

fn write(s: &(String, String)) {
    let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") else { return };
    let dir = format!("{rt}/spenmouse");
    let _ = std::fs::create_dir_all(&dir);
    // rename = readers never see half a file
    let tmp = format!("{dir}/status.tmp");
    if std::fs::write(&tmp, format!("phone={}\nstream={}\n", s.0, s.1)).is_ok() {
        let _ = std::fs::rename(&tmp, format!("{dir}/status"));
    }
}

/// Empty = no phone connected.
pub fn phone(ip: &str) {
    let mut s = STATE.lock().unwrap();
    if s.0 != ip {
        s.0 = ip.to_string();
        write(&s);
    }
}

/// Empty = no video.
pub fn stream(output: &str) {
    let mut s = STATE.lock().unwrap();
    if s.1 != output {
        s.1 = output.to_string();
        write(&s);
    }
}

pub fn reset() {
    write(&STATE.lock().unwrap());
}
