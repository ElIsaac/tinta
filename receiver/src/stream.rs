// Section 9: one encrypted H.264 stream over TCP, fed by a wf-recorder child.
use crate::engine::Engine;
use crate::proto::{self, Cipher};
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const VIRT: &str = "spenmouse";
const VIRT_GRACE: Duration = Duration::from_secs(5); // the phone reconnects within this without losing its monitor // name of the headless output of --screen virtual

pub enum Screen {
    Focused,
    Named(String),
    Virtual,
}

struct Active {
    id: u64,
    sock: TcpStream,
    child: Child,
}

pub struct Streams {
    engine: Arc<Mutex<Engine>>,
    cipher: Cipher,
    screen: Screen,
    scale: String,
    cur: Mutex<Option<Active>>, // the only video; whoever empties it kills the capture
    ids: AtomicU64,
}

pub fn hyprctl(args: &[&str]) -> io::Result<String> {
    let o = Command::new("hyprctl").args(args).stderr(Stdio::null()).output()?;
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Scale the phone asked for (U+E024), in percent; None = the --scale flag. Applies from the next STREAM.
static SCALE: Mutex<Option<u32>> = Mutex::new(None);

pub fn set_scale(percent: u32) {
    // 0 = back to the --scale flag
    if percent == 0 || (50..=400).contains(&percent) {
        *SCALE.lock().unwrap() = Some(percent).filter(|p| *p > 0);
    }
}

fn hyprctl_ok(args: &[&str]) -> io::Result<()> {
    let out = hyprctl(args)?;
    if out.trim() != "ok" {
        return Err(io::Error::other(format!("hyprctl {}: {}", args[..2].join(" "), out.trim())));
    }
    Ok(())
}

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

/// Hyprland reads Lua here: the monitor rule goes in through `eval` before the output exists,
/// so the output is born with the phone's size.
/// Largest size <= v that is even (H.264 4:2:0) and divides cleanly by the scale: Hyprland
/// rejects a scale that leaves a fractional logical size ("invalid scale passed to monitor").
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

pub fn virtual_exists() -> bool {
    hyprctl(&["monitors"]).is_ok_and(|o| o.lines().any(|l| l.starts_with(&format!("Monitor {VIRT} "))))
}

fn remove_virtual() {
    crate::ws::return_windows();
    crate::ws::give_back(true); // a lent workspace goes home, not wherever Hyprland drops it
    let _ = hyprctl(&["output", "remove", VIRT]);
}

/// Raw Annex-B on stdout, Intel VAAPI. -D = constant 60 fps even on a still screen (first
/// keyframe leaves at once), bf=0 = no B-frames, g=120 = IDR with SPS/PPS every 2 s.
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

fn stop(cur: &mut Option<Active>) {
    if let Some(mut a) = cur.take() {
        let _ = a.sock.shutdown(Shutdown::Both);
        let _ = a.child.kill();
        let _ = a.child.wait();
        crate::status::stream("");
        eprintln!("stream {}: stopped", a.id);
    }
}

impl Streams {
    pub fn new(engine: Arc<Mutex<Engine>>, key: &[u8; 16], screen: Screen, scale: String) -> Streams {
        if matches!(screen, Screen::Virtual) {
            remove_virtual(); // left behind by a receiver that was killed with SIGKILL
        }
        Streams { engine, cipher: proto::video_cipher(key), screen, scale, cur: Mutex::new(None), ids: AtomicU64::new(1) }
    }

    /// Stop the capture and exit; the lock is held so no new capture can start meanwhile.
    pub fn shutdown(&self, code: i32) -> ! {
        let mut cur = self.cur.lock().unwrap();
        stop(&mut cur);
        if matches!(self.screen, Screen::Virtual) {
            remove_virtual();
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
    fn watch(self: &Arc<Self>, id: u64, name: String) {
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

        let follow = matches!(self.screen, Screen::Focused);
        let (mut out, mut name, virt) = {
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
                    remove_virtual()
                }
            })?;
            let out = child.stdout.take().unwrap();
            eprintln!("stream {id}: {name} -> {} (phone {w}x{h})", sock.peer_addr()?);
            crate::status::stream(&name);
            *cur = Some(Active { id, sock: peer, child });
            (out, name, virt)
        };
        if follow {
            self.watch(id, name.clone());
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
                    let mut cur = self.cur.lock().unwrap();
                    let Some(a) = cur.as_mut().filter(|a| a.id == id) else { break };
                    let new = focused_output()?;
                    if new == name {
                        break; // the capture died on its own
                    }
                    let _ = a.child.wait();
                    let mut child = capture(&new).spawn()?;
                    out = child.stdout.take().unwrap();
                    a.child = child;
                    crate::status::stream(&new);
                    eprintln!("stream {id}: now {new}");
                    name = new;
                    self.watch(id, name.clone());
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
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::clean;

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
}
