// Section 9: one encrypted H.264 stream over TCP (wf-recorder on Linux, ScreenCaptureKit + VideoToolbox on macOS).
use crate::engine::Engine;
use crate::proto::{self, Cipher};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "linux")]
use std::process::{Child, ChildStdout, Command, Stdio};
#[cfg(target_os = "macos")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[allow(dead_code)]
pub const VIRT: &str = "spenmouse";
const VIRT_GRACE: Duration = Duration::from_secs(5); // the phone reconnects within this without losing its monitor // name of the headless output of --screen virtual

pub enum Screen {
    Focused,
    Named(String),
    Virtual,
    #[cfg(target_os = "macos")]
    Window,
}

#[cfg(target_os = "linux")]
struct Active {
    id: u64,
    sock: TcpStream,
    child: Child,
}

#[cfg(target_os = "linux")]
type CaptureOut = ChildStdout;
#[cfg(target_os = "linux")]
type WatchKey = String;

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum WatchKey {
    Display(u32),
    Window(i32, u32),
}

#[cfg(target_os = "macos")]
struct Active {
    id: u64,
    sock: TcpStream,
    cap: Arc<crate::macos::CaptureHandle>,
    key: WatchKey,
}

#[cfg(target_os = "macos")]
type CaptureOut = crate::macos::MacCapture;

pub struct Streams {
    engine: Arc<Mutex<Engine>>,
    cipher: Cipher,
    screen: Screen,
    scale: String,
    cur: Mutex<Option<Active>>, // the only video; whoever empties it kills the capture
    ids: AtomicU64,
}

/// Scale the phone asked for (U+E024), in percent; None = the --scale flag. Applies from the next STREAM.
static SCALE: Mutex<Option<u32>> = Mutex::new(None);

pub fn set_scale(percent: u32) {
    // 0 = back to the --scale flag
    if percent == 0 || (50..=400).contains(&percent) {
        *SCALE.lock().unwrap() = Some(percent).filter(|p| *p > 0);
    }
}

/// Hyprland reads Lua here: the monitor rule goes in through `eval` before the output exists,
/// so the output is born with the phone's size.
/// Largest size <= v that is even (H.264 4:2:0) and divides cleanly by the scale: Hyprland
/// rejects a scale that leaves a fractional logical size ("invalid scale passed to monitor").
#[cfg(any(target_os = "linux", test))]
fn clean(v: u16, scale: &str) -> u16 {
    let k: f32 = scale.parse().unwrap_or(1.0);
    let mut v = (v & !1).clamp(320, 4096);
    for _ in 0..64 {
        let logical = v as f32 / k;
        if (logical - logical.round()).abs() < 0.001 {
            break;
        }
        v -= 2;
    }
    v
}

#[cfg(target_os = "linux")]
pub fn hyprctl(args: &[&str]) -> io::Result<String> {
    let o = Command::new("hyprctl").args(args).stderr(Stdio::null()).output()?;
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(target_os = "linux")]
fn hyprctl_ok(args: &[&str]) -> io::Result<()> {
    let out = hyprctl(args)?;
    if out.trim() != "ok" {
        return Err(io::Error::other(format!("hyprctl {}: {}", args[..2].join(" "), out.trim())));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn focused_output() -> io::Result<String> {
    let out = hyprctl(&["monitors"])?;
    let mut name = None;
    for l in out.lines() {
        if let Some(r) = l.strip_prefix("Monitor ") {
            name = r.split(' ').next();
        } else if l.trim() == "focused: yes" {
            break;
        }
    }
    name.map(str::to_string).ok_or_else(|| io::Error::other("hyprctl monitors: no focused monitor"))
}

#[cfg(target_os = "linux")]
fn create_virtual(w: u16, h: u16, scale: &str) -> io::Result<()> {
    let (w, h) = (clean(w, scale), clean(h, scale));
    let rule = format!("hl.monitor({{output=\"{VIRT}\", mode=\"{w}x{h}@60\", position=\"auto\", scale=\"{scale}\"}})");
    hyprctl_ok(&["eval", &rule])?; // on an existing output this resizes it live (phone rotated)
    // the phone's touch and absolute pen land on its own monitor, not on the whole desktop
    let _ = hyprctl(&["eval", &format!("hl.config({{[\"input.touchdevice.output\"]=\"{VIRT}\", [\"input.tablet.output\"]=\"{VIRT}\"}})")]);
    if virtual_exists() {
        return Ok(());
    }
    hyprctl_ok(&["output", "create", "headless", VIRT])
}

#[cfg(target_os = "linux")]
pub fn virtual_exists() -> bool {
    hyprctl(&["monitors"]).is_ok_and(|o| o.lines().any(|l| l.starts_with(&format!("Monitor {VIRT} "))))
}

#[cfg(target_os = "linux")]
fn remove_virtual() {
    crate::ws::return_windows();
    crate::ws::give_back(true); // a lent workspace goes home, not wherever Hyprland drops it
    let _ = hyprctl(&["output", "remove", VIRT]);
}

/// Raw Annex-B on stdout, Intel VAAPI. -D = constant 60 fps even on a still screen (first
/// keyframe leaves at once), bf=0 = no B-frames, g=120 = IDR with SPS/PPS every 2 s.
#[cfg(target_os = "linux")]
fn capture(output: &str) -> Command {
    let mut c = Command::new("wf-recorder");
    c.args(["-y", "-D", "-o", output, "-r", "60", "-c", "h264_vaapi", "-d", "/dev/dri/renderD128"])
        .args(["-p", "bf=0", "-p", "g=120", "-p", "rc_mode=VBR", "-p", "b=8M", "-p", "maxrate=12M"])
        .args(["-p", "async_depth=1", "-m", "h264", "-f", "/dev/stdout"])
        // the session exports LIBVA_DRIVER_NAME=nvidia, which has no VAAPI driver here
        .env("LIBVA_DRIVER_NAME", "iHD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        c.pre_exec(|| {
            // dies with us even on SIGKILL/abort (tied to the spawning thread, which is the
            // relay thread and outlives the child) and does not inherit our blocked signals
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
            Ok(())
        });
    }
    c
}

#[cfg(target_os = "linux")]
fn stop(cur: &mut Option<Active>) {
    if let Some(mut a) = cur.take() {
        let _ = a.sock.shutdown(Shutdown::Both);
        let _ = a.child.kill();
        let _ = a.child.wait();
        crate::status::stream("");
        eprintln!("stream {}: stopped", a.id);
    }
}

#[cfg(target_os = "macos")]
fn create_virtual(w: u16, h: u16, scale: &str) -> (u32, bool) {
    let k: f32 = scale.parse().unwrap_or(1.5);
    let did = crate::macos::virtual_ensure(w as u32, h as u32, k);
    if did != 0 {
        (did, true)
    } else {
        eprintln!("stream: virtual display unavailable, falling back to main display");
        let fallback = crate::macos::find_display("main").unwrap_or_else(crate::macos::focused_display);
        (fallback, false)
    }
}

#[cfg(target_os = "macos")]
fn remove_virtual() {
    crate::ws::return_windows();
    crate::ws::give_back(true);
    crate::macos::virtual_remove();
}

#[cfg(target_os = "macos")]
type AdaptedWin = (i32, u32, f64, f64, f64, f64);
#[cfg(target_os = "macos")]
static WINDOW_MODE: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static WANTED_WIN: Mutex<Option<(i32, u32)>> = Mutex::new(None);
#[cfg(target_os = "macos")]
static ADAPTED_WIN: Mutex<Option<AdaptedWin>> = Mutex::new(None);

#[cfg(target_os = "macos")]
pub fn is_window_mode() -> bool {
    WINDOW_MODE.load(Ordering::Relaxed)
}

#[cfg(target_os = "macos")]
pub fn select_window(pid: i32, wid: u32) {
    if pid > 0 && wid > 0 {
        *WANTED_WIN.lock().unwrap() = Some((pid, wid));
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn compute_window_rect(dx: f64, dy: f64, dw: f64, dh: f64, phone_w: u16, phone_h: u16) -> (f64, f64, f64, f64) {
    let avail_w = (dw - 24.0).max(420.0);
    let avail_h = (dh - 88.0).max(360.0);
    let aspect = (phone_w.max(1) as f64) / (phone_h.max(1) as f64);
    let mut th = avail_h;
    let mut tw = th * aspect;
    if tw > avail_w {
        tw = avail_w;
        th = tw / aspect;
    }
    tw = tw.clamp(420.0_f64.min(avail_w), avail_w);
    th = th.clamp(360.0_f64.min(avail_h), avail_h);
    let tx = dx + ((dw - tw) / 2.0).max(0.0);
    let ty = dy + 32.0;
    (tx.round(), ty.round(), tw.round(), th.round())
}

#[cfg(target_os = "macos")]
fn adapt_window(pid: i32, wid: u32, phone_w: u16, phone_h: u16) {
    let old = {
        let mut adapted = ADAPTED_WIN.lock().unwrap();
        match *adapted {
            Some((opid, owid, ox, oy, ow, oh)) if (opid, owid) != (pid, wid) => {
                *adapted = None;
                Some((opid, owid, ox, oy, ow, oh))
            }
            _ => None,
        }
    };
    if let Some((opid, owid, ox, oy, ow, oh)) = old {
        crate::macos::move_window(opid, owid as i32, ox, oy, ow, oh, false);
    }

    let did = crate::macos::display_for_window(wid);
    if let Some((dx, dy, dw, dh)) = crate::macos::display_bounds(did) {
        let (tx, ty, tw, th) = compute_window_rect(dx, dy, dw, dh, phone_w, phone_h);
        if let Some((ox, oy, ow, oh)) =
            crate::macos::move_window(pid, wid as i32, tx, ty, tw, th, true)
        {
            let mut adapted = ADAPTED_WIN.lock().unwrap();
            if adapted.is_none() {
                *adapted = Some((pid, wid, ox, oy, ow, oh));
            }
        }
    } else {
        crate::macos::focus_window(pid, wid as i32);
    }
    std::thread::sleep(Duration::from_millis(60));
}

#[cfg(target_os = "macos")]
fn restore_adapted_window() {
    let saved = ADAPTED_WIN.lock().unwrap().take();
    if let Some((pid, wid, ox, oy, ow, oh)) = saved {
        crate::macos::move_window(pid, wid as i32, ox, oy, ow, oh, false);
    }
}

#[cfg(target_os = "macos")]
fn start_window_capture(pid: i32, wid: u32, w: u16, h: u16) -> io::Result<(CaptureOut, WatchKey, String)> {
    *WANTED_WIN.lock().unwrap() = Some((pid, wid));
    adapt_window(pid, wid, w, h);
    let cap = crate::macos::MacCapture::start_window(wid, w as u32, h as u32)
        .inspect_err(|_| restore_adapted_window())?;
    crate::macos::set_target_window(wid);
    crate::macos::set_target_display(0);
    let label = crate::macos::window_label(wid);
    crate::status::stream(&label);
    Ok((cap, WatchKey::Window(pid, wid), label))
}

#[cfg(target_os = "macos")]
fn stop(cur: &mut Option<Active>) {
    if let Some(a) = cur.take() {
        let _ = a.sock.shutdown(Shutdown::Both);
        a.cap.stop();
        crate::macos::set_target_window(0);
        crate::macos::set_target_display(0);
        crate::macos::prevent_sleep(false);
        crate::status::stream("");
        eprintln!("stream {}: stopped", a.id);
    }
}

impl Streams {
    pub fn new(engine: Arc<Mutex<Engine>>, key: &[u8; 16], screen: Screen, scale: String) -> Streams {
        if matches!(screen, Screen::Virtual) {
            remove_virtual(); // left behind by a receiver that was killed with SIGKILL
        }
        #[cfg(target_os = "macos")]
        WINDOW_MODE.store(matches!(screen, Screen::Window), Ordering::Relaxed);
        Streams { engine, cipher: proto::video_cipher(key), screen, scale, cur: Mutex::new(None), ids: AtomicU64::new(1) }
    }

    /// Stop the capture and exit; the lock is held so no new capture can start meanwhile.
    pub fn shutdown(&self, code: i32) -> ! {
        let mut cur = self.cur.lock().unwrap();
        stop(&mut cur);
        if matches!(self.screen, Screen::Virtual) {
            remove_virtual();
        }
        #[cfg(target_os = "macos")]
        if matches!(self.screen, Screen::Window) {
            restore_adapted_window();
        }
        std::process::exit(code)
    }

    pub fn serve(self: Arc<Self>, listener: TcpListener) {
        for sock in listener.incoming().flatten() {
            let st = self.clone();
            // a thread per connection: a client that never sends its STREAM cannot hold up a real one
            std::thread::spawn(move || {
                if let Err(e) = st.handle(sock) {
                    eprintln!("stream: {e}");
                }
            });
        }
    }

    /// Focused mode: when the focus moves to another monitor, stop the capture gracefully; the
    /// relay sees EOF and restarts it on the new monitor over the same connection.
    // shortcut: polls hyprctl every 150 ms, switch to Hyprland's event socket if that ever costs.
    #[cfg(target_os = "linux")]
    fn watch(self: &Arc<Self>, id: u64, key: &WatchKey) {
        let name = key.clone();
        let st = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(150));
            let now = focused_output().ok();
            let cur = st.cur.lock().unwrap();
            let Some(a) = cur.as_ref().filter(|a| a.id == id) else { return };
            if now.as_deref().is_some_and(|n| n != name) {
                // SIGINT: wf-recorder flushes whole frames and exits
                unsafe { libc::kill(a.child.id() as i32, libc::SIGINT) };
                return;
            }
        });
    }

    #[cfg(target_os = "linux")]
    fn start_active(&self, id: u64, peer: TcpStream, w: u16, h: u16) -> io::Result<(CaptureOut, WatchKey, bool)> {
        let mut cur = self.cur.lock().unwrap();
        stop(&mut cur); // a valid STREAM replaces the previous one
        let (name, virt) = match &self.screen {
            Screen::Named(n) => (n.clone(), false),
            Screen::Focused => (focused_output()?, false),
            Screen::Virtual => {
                let scale = SCALE.lock().unwrap().map_or(self.scale.clone(), |p| format!("{:.2}", p as f32 / 100.0));
                create_virtual(w, h, &scale).inspect_err(|_| remove_virtual())?;
                (VIRT.to_string(), true)
            }
        };
        let mut child = capture(&name).spawn().inspect_err(|_| {
            if virt {
                remove_virtual();
            }
        })?;
        let out = child.stdout.take().unwrap();
        eprintln!("stream {id}: {name} -> {} (phone {w}x{h})", peer.peer_addr()?);
        crate::status::stream(&name);
        *cur = Some(Active { id, sock: peer, child });
        Ok((out, name, virt))
    }

    #[cfg(target_os = "linux")]
    fn restart_active(&self, id: u64, name: &str, _w: u16, _h: u16) -> io::Result<Option<(CaptureOut, WatchKey)>> {
        let mut cur = self.cur.lock().unwrap();
        let Some(a) = cur.as_mut().filter(|a| a.id == id) else { return Ok(None) };
        let new = focused_output()?;
        if new == name {
            return Ok(None); // the capture died on its own
        }
        let _ = a.child.wait();
        let mut child = capture(&new).spawn()?;
        let out = child.stdout.take().unwrap();
        a.child = child;
        crate::status::stream(&new);
        eprintln!("stream {id}: now {new}");
        Ok(Some((out, new)))
    }

    #[cfg(target_os = "macos")]
    fn watch(self: &Arc<Self>, id: u64, key: &WatchKey) {
        let key = *key;
        let st = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(150));
            let stop_now = match key {
                WatchKey::Display(display_id) => crate::macos::focused_display() != display_id,
                WatchKey::Window(pid, wid) => {
                    WANTED_WIN.lock().unwrap().is_some_and(|w| w != (pid, wid))
                        || !crate::macos::window_exists(wid)
                }
            };
            let cur = st.cur.lock().unwrap();
            let Some(a) = cur.as_ref().filter(|a| a.id == id) else { return };
            if stop_now {
                a.cap.stop();
                return;
            }
        });
    }

    #[cfg(target_os = "macos")]
    fn start_active(&self, id: u64, peer: TcpStream, w: u16, h: u16) -> io::Result<(CaptureOut, WatchKey, bool)> {
        let mut cur = self.cur.lock().unwrap();
        stop(&mut cur); // a valid STREAM replaces the previous one
        let peer_addr = peer.peer_addr()?;
        if matches!(self.screen, Screen::Window) {
            let wanted = *WANTED_WIN.lock().unwrap();
            let (pid, wid) = wanted
                .filter(|&(_, wid)| crate::macos::window_exists(wid))
                .or_else(crate::macos::frontmost_window)
                .ok_or_else(|| io::Error::other("no capturable window"))?;
            let (cap, key, label) = start_window_capture(pid, wid, w, h)?;
            crate::macos::prevent_sleep(true);
            eprintln!("stream {id}: window {pid}:{wid} ({label}) -> {peer_addr} (phone {w}x{h})");
            let stopper = cap.handle();
            *cur = Some(Active { id, sock: peer, cap: stopper, key });
            return Ok((cap, key, false));
        }
        let (display_id, virt) = match &self.screen {
            Screen::Named(n) => (
                crate::macos::find_display(n).ok_or_else(|| io::Error::other(format!("display not found: {n}")))?,
                false,
            ),
            Screen::Focused => (crate::macos::focused_display(), false),
            Screen::Virtual => {
                let scale = SCALE.lock().unwrap().map_or_else(|| self.scale.clone(), |p| format!("{:.2}", p as f32 / 100.0));
                let (did, virt) = create_virtual(w, h, &scale);
                if virt {
                    crate::ws::refit_virtual();
                }
                (did, virt)
            }
            Screen::Window => unreachable!(),
        };
        let cap = crate::macos::MacCapture::start(display_id, w as u32, h as u32).inspect_err(|_| {
            if virt {
                remove_virtual();
            }
        })?;
        crate::macos::set_target_window(0);
        crate::macos::set_target_display(display_id);
        crate::macos::prevent_sleep(true);
        let name = crate::macos::display_name(display_id);
        eprintln!("stream {id}: {name} -> {peer_addr} (phone {w}x{h})");
        crate::status::stream(&name);
        let stopper = cap.handle();
        let key = WatchKey::Display(display_id);
        *cur = Some(Active { id, sock: peer, cap: stopper, key });
        Ok((cap, key, virt))
    }

    #[cfg(target_os = "macos")]
    fn restart_active(&self, id: u64, _prev_key: &WatchKey, w: u16, h: u16) -> io::Result<Option<(CaptureOut, WatchKey)>> {
        let mut cur = self.cur.lock().unwrap();
        let Some(a) = cur.as_mut().filter(|a| a.id == id) else { return Ok(None) };
        match a.key {
            WatchKey::Display(prev_id) => {
                let new_id = crate::macos::focused_display();
                if new_id == prev_id {
                    return Ok(None); // the capture died on its own
                }
                let cap = crate::macos::MacCapture::start(new_id, w as u32, h as u32)?;
                let new_key = WatchKey::Display(new_id);
                a.cap = cap.handle();
                a.key = new_key;
                crate::macos::set_target_window(0);
                crate::macos::set_target_display(new_id);
                let new_name = crate::macos::display_name(new_id);
                crate::status::stream(&new_name);
                eprintln!("stream {id}: now {new_name}");
                Ok(Some((cap, new_key)))
            }
            WatchKey::Window(prev_pid, prev_wid) => {
                let wanted = *WANTED_WIN.lock().unwrap();
                let next = match wanted {
                    Some((wpid, wwid)) if (wpid, wwid) != (prev_pid, prev_wid) && crate::macos::window_exists(wwid) => {
                        Some((wpid, wwid))
                    }
                    _ if !crate::macos::window_exists(prev_wid) => crate::macos::frontmost_window(),
                    _ => None,
                };
                let Some((new_pid, new_wid)) = next.filter(|&k| k != (prev_pid, prev_wid)) else {
                    return Ok(None);
                };
                let (cap, new_key, label) = start_window_capture(new_pid, new_wid, w, h)?;
                a.cap = cap.handle();
                a.key = new_key;
                eprintln!("stream {id}: now window {new_pid}:{new_wid} ({label})");
                Ok(Some((cap, new_key)))
            }
        }
    }

    fn handle(self: &Arc<Self>, mut sock: TcpStream) -> io::Result<()> {
        sock.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut req = [0u8; 28];
        if sock.read_exact(&mut req).is_err() {
            return Ok(());
        }
        let Some((w, h)) = self.engine.lock().unwrap().on_stream(&req) else {
            return Ok(()); // close without answering
        };
        sock.set_nodelay(true)?;
        // a phone that vanished without FIN must not keep the capture alive
        sock.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut np = [0u8; 4];
        File::open("/dev/urandom")?.read_exact(&mut np)?;
        let peer = sock.try_clone()?;
        let id = self.ids.fetch_add(1, Ordering::Relaxed);

        let follow = matches!(self.screen, Screen::Focused) || {
            #[cfg(target_os = "macos")]
            {
                matches!(self.screen, Screen::Window)
            }
            #[cfg(not(target_os = "macos"))]
            {
                false
            }
        };
        let (mut out, mut watch_key, virt) = self.start_active(id, peer, w, h)?;
        if follow {
            self.watch(id, &watch_key);
        }

        // a write error here is just the phone closing (or being replaced): not worth a log line
        let _: io::Result<()> = (|| {
            sock.write_all(&np)?;
            let mut buf = vec![0u8; 64 * 1024]; // one pipe read = one record, far below 1 MiB
            let mut n = 0u64;
            loop {
                let len = out.read(&mut buf)?;
                if len == 0 {
                    if !follow {
                        break;
                    }
                    let Some((new_out, new_key)) = self.restart_active(id, &watch_key, w, h)? else {
                        break;
                    };
                    out = new_out;
                    watch_key = new_key;
                    self.watch(id, &watch_key);
                    continue;
                }
                sock.write_all(&proto::record(&self.cipher, np, n, &buf[..len]))?;
                n += 1;
            }
            Ok(())
        })();
        let mut cur = self.cur.lock().unwrap();
        if cur.as_ref().is_some_and(|a| a.id == id) {
            stop(&mut cur); // not replaced: the connection or the capture ended on its own
            if virt {
                let st = self.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(VIRT_GRACE);
                    if st.cur.lock().unwrap().is_none() {
                        remove_virtual();
                    }
                });
            }
            #[cfg(target_os = "macos")]
            if matches!(self.screen, Screen::Window) {
                let st = self.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(VIRT_GRACE);
                    if st.cur.lock().unwrap().is_none() {
                        restore_adapted_window();
                        *WANTED_WIN.lock().unwrap() = None;
                    }
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{clean, compute_window_rect};

    #[test]
    fn sizes_divide_cleanly_by_the_scale() {
        assert_eq!(clean(2340, "1.5"), 2340);
        assert_eq!(clean(945, "1.5"), 942);
        assert_eq!(clean(1080, "1.5"), 1080);
        assert_eq!(clean(2205, "1.5"), 2202);
        assert_eq!(clean(945, "1"), 944);
        assert_eq!(clean(945, "1.25"), 940);
        assert_eq!(clean(100, "2"), 320);
    }

    #[test]
    fn adaptive_window_rect_fits_phone_aspect_and_clamps_small_displays() {
        // Portrait phone on large landscape display: fits height and centers horizontally.
        assert_eq!(
            compute_window_rect(0.0, 0.0, 2560.0, 1440.0, 1080, 2340),
            (968.0, 32.0, 624.0, 1352.0)
        );
        // Portrait phone on 1512x982 display: clamps width up to 420 minimum.
        assert_eq!(
            compute_window_rect(0.0, 0.0, 1512.0, 982.0, 1080, 2340),
            (546.0, 32.0, 420.0, 894.0)
        );
        // Landscape phone on landscape display with non-zero display origin.
        assert_eq!(
            compute_window_rect(100.0, 50.0, 1512.0, 982.0, 2340, 1080),
            (112.0, 82.0, 1488.0, 687.0)
        );
        // Small display: clamps to 420x360 minimums without negative horizontal offset.
        assert_eq!(
            compute_window_rect(0.0, 0.0, 400.0, 300.0, 1080, 2340),
            (0.0, 32.0, 420.0, 360.0)
        );
    }
}
