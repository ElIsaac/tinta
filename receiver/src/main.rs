mod clip;
mod engine;
mod input;
#[cfg(target_os = "macos")]
mod macos;
mod proto;
mod status;
mod stream;
mod text;
#[cfg(target_os = "linux")]
mod uinput;
mod ws;

use engine::{Engine, Frame, Reply};
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

type Res<T> = Result<T, Box<dyn Error>>;

const FAILSAFE: Duration = Duration::from_millis(500);
const USAGE: &str = "\
spenmouse-receiver [--port N] [--sensitivity F] [--screen virtual|focused|window|NAME] [--scale F] [--dry-run]
spenmouse-receiver pair [--port N] [--host IPV4] [--alt IPV4]...

  --port N         UDP port, and TCP port of the video stream (default 47800)
  --sensitivity F  finger movement, mouse counts per mm (default 20)
  --screen WHAT    what the phone shows: `virtual` (default) = a third monitor with the phone's
                   size, alive while the phone streams; `focused` = mirror the focused
                   monitor; `window` = mirror and adapt a single window to the phone (macOS);
                   NAME = mirror that output
  --scale F        scale of the virtual monitor (default 1.5)
  --dry-run        print events and text instead of injecting them (/dev/uinput, wtype)
  --host IPV4      address to put in the pairing URI (default: default-route interface)
  --alt IPV4       another address of this laptop for the phone to try (VPN, other network); repeatable";

fn key_path() -> Res<PathBuf> {
    Ok(PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".config/spenmouse/key"))
}

fn load_or_create_key() -> Res<[u8; 16]> {
    let path = key_path()?;
    let mut key = [0u8; 16];
    match fs::read_to_string(&path) {
        Ok(s) => {
            let s = s.trim();
            let ok = s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if !ok {
                return Err(format!("{}: expected 32 lowercase hex chars (delete it to rotate)", path.display()).into());
            }
            for (i, k) in key.iter_mut().enumerate() {
                *k = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)?;
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            File::open("/dev/urandom")?.read_exact(&mut key)?;
            fs::DirBuilder::new().recursive(true).mode(0o700).create(path.parent().unwrap())?;
            let mut f = File::options().write(true).create_new(true).mode(0o600).open(&path)?;
            f.write_all(hex(&key).as_bytes())?;
            eprintln!("new key written to {}", path.display());
        }
        Err(e) => return Err(format!("{}: {e}", path.display()).into()),
    }
    Ok(key)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid writable storage of length 256.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) } != 0 {
        return String::new();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len])
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
        .collect()
}

fn pair(port: u16, host: Option<String>, alts: Vec<String>) -> Res<()> {
    let key = load_or_create_key()?;
    let host = match host {
        Some(h) => h.parse::<std::net::Ipv4Addr>().map_err(|_| "--host must be an IPv4 address")?.to_string(),
        None => {
            // connect() on UDP only picks a route; nothing is sent
            let s = UdpSocket::bind("0.0.0.0:0")?;
            s.connect("1.1.1.1:9").map_err(|e| format!("no default route ({e}); use --host"))?;
            s.local_addr()?.ip().to_string()
        }
    };
    let name = hostname();
    let mut uri = format!("spenmouse://pair?host={host}&port={port}&key={}&name={name}", hex(&key));
    // other addresses of this laptop (VPN, another network): the phone tries them all
    for a in &alts {
        a.parse::<std::net::Ipv4Addr>().map_err(|_| "--alt must be an IPv4 address")?;
    }
    if !alts.is_empty() {
        uri += &format!("&alt={}", alts.join(","));
    }
    // the MAC on the current network, so the phone can wake this laptop before ever connecting
    if let Some(mac) = host.parse().ok().and_then(mac_of_local) {
        uri += &format!("&mac={mac}");
    }
    let qr = qrcode::QrCode::new(uri.as_bytes())?;
    println!("{}", qr.render::<qrcode::render::unicode::Dense1x2>().build());
    println!("\n{uri}\n");
    // the device shell re-parses the command, so the URI needs its own quotes there
    println!("adb shell \"am start -a android.intent.action.VIEW -d '{uri}'\"");
    Ok(())
}

enum Sink {
    Dry,
    Real { mouse: input::Mouse, tablet: Option<input::Tablet>, touch: Option<input::Touch> },
}

impl Sink {
    fn frames(&mut self, tablet: &Frame, mouse: &Frame, touch: &Frame) -> io::Result<()> {
        match self {
            Sink::Dry => {
                for (dev, f) in [("tablet", tablet), ("mouse", mouse), ("touch", touch)] {
                    if f.n > 0 {
                        let mut line = String::from(dev);
                        for e in f.events() {
                            line += &format!(" {}={}", ev_name(e.kind, e.code), e.value);
                        }
                        println!("{line}");
                    }
                }
            }
            Sink::Real { mouse: m, tablet: t, touch: th } => {
                if let (true, Some(t)) = (tablet.n > 0, t) {
                    input::write_tablet(t, tablet)?;
                }
                if let (true, Some(th)) = (touch.n > 0, th) {
                    input::write_touch(th, touch)?;
                }
                if mouse.n > 0 {
                    input::write_mouse(m, mouse)?;
                }
            }
        }
        Ok(())
    }
}

fn ev_name(kind: u16, code: u16) -> &'static str {
    use engine::*;
    match (kind, code) {
        (EV_ABS, ABS_X) => "ABS_X",
        (EV_ABS, ABS_Y) => "ABS_Y",
        (EV_ABS, ABS_PRESSURE) => "ABS_PRESSURE",
        (EV_REL, REL_X) => "REL_X",
        (EV_REL, REL_Y) => "REL_Y",
        (EV_REL, REL_WHEEL) => "REL_WHEEL",
        (EV_KEY, BTN_LEFT) => "BTN_LEFT",
        (EV_KEY, BTN_RIGHT) => "BTN_RIGHT",
        (EV_KEY, BTN_MIDDLE) => "BTN_MIDDLE",
        (EV_KEY, BTN_TOOL_PEN) => "BTN_TOOL_PEN",
        (EV_KEY, BTN_TOUCH) => "BTN_TOUCH",
        (EV_KEY, BTN_STYLUS) => "BTN_STYLUS",
        _ => "?",
    }
}

/// SIGINT/SIGTERM/SIGHUP are blocked in every thread and taken by one sigwait thread, so the
/// capture process and the virtual output are cleaned up before exiting.
fn exit_on_signal(streams: Arc<stream::Streams>) {
    // SAFETY: plain libc signal-set calls on a local set
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    std::thread::spawn(move || {
        let mut sig = 0;
        unsafe { libc::sigwait(&set, &mut sig) };
        streams.shutdown(128 + sig)
    });
}

/// MAC of the interface that owns this local address.
#[cfg(target_os = "linux")]
fn mac_of_local(ip: std::net::Ipv4Addr) -> Option<String> {
    let out = std::process::Command::new("ip").args(["-o", "-4", "addr", "show"]).output().ok()?;
    let txt = String::from_utf8_lossy(&out.stdout).into_owned();
    // "3: wlp0s20f3    inet 192.168.0.28/24 brd ..."
    let dev = txt.lines().find(|l| l.contains(&format!("inet {ip}/")))?.split_whitespace().nth(1)?.to_string();
    Some(fs::read_to_string(format!("/sys/class/net/{dev}/address")).ok()?.trim().to_string())
}

#[cfg(target_os = "macos")]
fn mac_of_local(ip: std::net::Ipv4Addr) -> Option<String> {
    struct IfAddrs(*mut libc::ifaddrs);
    impl Drop for IfAddrs {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { libc::freeifaddrs(self.0) };
            }
        }
    }

    let mut raw: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut raw) } != 0 || raw.is_null() {
        return None;
    }
    let guard = IfAddrs(raw);

    let mut if_name: Option<Vec<u8>> = None;
    let mut cur = guard.0;
    while !cur.is_null() {
        // SAFETY: node in getifaddrs linked list is valid until freeifaddrs on drop.
        let ifa = unsafe { &*cur };
        if !ifa.ifa_addr.is_null() && !ifa.ifa_name.is_null() {
            let family = unsafe { (*ifa.ifa_addr).sa_family as i32 };
            if family == libc::AF_INET {
                let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                let addr = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if addr == ip {
                    let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) };
                    if_name = Some(name.to_bytes().to_vec());
                    break;
                }
            }
        }
        cur = ifa.ifa_next;
    }
    let target = if_name?;

    let mut cur = guard.0;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        if !ifa.ifa_addr.is_null() && !ifa.ifa_name.is_null() {
            let family = unsafe { (*ifa.ifa_addr).sa_family as i32 };
            if family == libc::AF_LINK {
                let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) };
                if name.to_bytes() == target.as_slice() {
                    let sdl = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_dl) };
                    if sdl.sdl_alen == 6 {
                        let data_off = (sdl.sdl_data.as_ptr() as usize) - (sdl as *const _ as usize);
                        if (sdl.sdl_len as usize) >= data_off + sdl.sdl_nlen as usize + 6 {
                            let ptr = unsafe { (sdl.sdl_data.as_ptr() as *const u8).add(sdl.sdl_nlen as usize) };
                            let m = unsafe { std::slice::from_raw_parts(ptr, 6) };
                            return Some(format!(
                                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                                m[0], m[1], m[2], m[3], m[4], m[5]
                            ));
                        }
                    }
                }
            }
        }
        cur = ifa.ifa_next;
    }
    None
}

/// Nonce counter of WSLIST packets: never repeats within a session.
fn next_ws_seq() -> u32 {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed).wrapping_add(1)
}

/// MAC of the interface that routes to `ip`: the one a magic packet from there must carry.
#[cfg(target_os = "linux")]
fn mac_towards(ip: std::net::IpAddr) -> Option<String> {
    let out = std::process::Command::new("ip").args(["-o", "route", "get", &ip.to_string()]).output().ok()?;
    let route = String::from_utf8_lossy(&out.stdout).into_owned();
    let dev = route.split_whitespace().skip_while(|w| *w != "dev").nth(1)?;
    Some(fs::read_to_string(format!("/sys/class/net/{dev}/address")).ok()?.trim().to_string())
}

#[cfg(target_os = "macos")]
fn mac_towards(ip: std::net::IpAddr) -> Option<String> {
    let std::net::IpAddr::V4(v4) = ip else { return None };
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect((v4, 9)).ok()?;
    let std::net::SocketAddr::V4(local) = s.local_addr().ok()? else { return None };
    mac_of_local(*local.ip())
}

fn run(port: u16, sens: f32, dry: bool, screen: stream::Screen, scale: String) -> Res<()> {
    let key = load_or_create_key()?;
    let mut sink = if dry {
        Sink::Dry
    } else {
        #[cfg(target_os = "macos")]
        macos::check_permissions();
        let mouse = input::mouse().map_err(|e| {
            #[cfg(target_os = "linux")]
            { format!("/dev/uinput: {e} (needs write access for your user, or use --dry-run)") }
            #[cfg(not(target_os = "linux"))]
            { format!("input: {e}") }
        })?;
        Sink::Real { mouse, tablet: None, touch: None }
    };
    let sock = UdpSocket::bind(("0.0.0.0", port))?;
    let tcp = TcpListener::bind(("0.0.0.0", port))?;
    eprintln!("listening on udp+tcp 0.0.0.0:{port}{}", if dry { " (dry run)" } else { "" });

    // One engine for the UDP loop and the TCP path: session and last_seq are shared.
    let shared = Arc::new(Mutex::new(Engine::new(&key, sens)));
    let streams = Arc::new(stream::Streams::new(shared.clone(), &key, screen, scale));
    if !dry {
        ws::watch();
        status::reset();
    }
    exit_on_signal(streams.clone()); // before any other thread, so all inherit the mask
    std::thread::spawn(move || streams.serve(tcp));
    // wtype runs in its own thread, one text at a time: order kept, pointer never waits
    let (typist, texts) = mpsc::channel::<String>();
    let seen = Arc::new(Mutex::new(Instant::now())); // last valid HELLO
    let kbd = if dry { None } else { Some(input::keyboard()?) };
    let seen_typist = seen.clone();
    std::thread::spawn(move || {
        let mut typist = text::Typist::new(kbd);
        loop {
            match texts.recv_timeout(Duration::from_secs(1)) {
                Ok(t) => typist.type_text(&t),
                // a phone that vanished must not leave a modifier held on the laptop
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if seen_typist.lock().unwrap().elapsed() > Duration::from_secs(3) {
                        status::phone(""); // the phone stopped saying HELLO
                    }
                    if typist.any_held() && seen_typist.lock().unwrap().elapsed() > Duration::from_secs(3) {
                        typist.release_all();
                        eprintln!("failsafe: no HELLO for 3 s, released the modifiers");
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let kws = proto::ws_cipher(&key);
    // Shared clipboard: every change on the laptop goes to the phone that is connected.
    let phone_addr: Arc<Mutex<Option<std::net::SocketAddr>>> = Arc::new(Mutex::new(None));
    let send_clip: Arc<dyn Fn(String) + Send + Sync> = {
        let (out, kws, engine, phone) = (sock.try_clone()?, kws.clone(), shared.clone(), phone_addr.clone());
        Arc::new(move |text: String| {
            let (Some(to), session) = (*phone.lock().unwrap(), engine.lock().unwrap().session()) else { return };
            for part in clip::packets(&text) {
                let _ = out.send_to(&proto::wslist(&kws, next_ws_seq(), session, &part), to);
            }
        })
    };
    if !dry {
        let send = send_clip.clone();
        clip::watch(move |text| send(text));
    }
    let mut urandom = File::open("/dev/urandom")?;
    let mut new_session = move || {
        let mut b = [0u8; 8];
        urandom.read_exact(&mut b).expect("/dev/urandom");
        u64::from_be_bytes(b)
    };
    let mut tablet_pad = (0u16, 0u16);
    let mut last_accept = Instant::now();
    let mut armed = false; // socket has a read timeout that ends at last_accept + FAILSAFE
    let mut buf = [0u8; proto::TEXT_MAX + 1]; // +1: an oversized datagram shows as a bad length
    #[cfg(target_os = "macos")]
    let ime_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
    #[cfg(target_os = "macos")]
    let ime_sock = Arc::new(sock.try_clone()?);

    loop {
        let (mut tf, mut mf) = (Frame::default(), Frame::default());
        let mut accepted = false;
        let got = sock.recv_from(&mut buf);
        let mut engine = shared.lock().unwrap(); // never held while blocked in recv
        match got {
            Ok((n, from)) => match engine.on_packet(&buf[..n], &mut new_session, &mut tf, &mut mf) {
                Reply::Accepted => {
                    accepted = true;
                    last_accept = Instant::now();
                }
                Reply::Welcome(w) => {
                    let _ = sock.send_to(&w, from);
                    *seen.lock().unwrap() = Instant::now();
                    *phone_addr.lock().unwrap() = Some(from);
                    if !dry {
                        status::phone(&from.ip().to_string());
                    }
                    // The tablet needs a real resolution and pad size only arrives in HELLO,
                    // so it is created on the first HELLO and recreated if the pad size changes.
                    if engine.pad != tablet_pad {
                        tablet_pad = engine.pad;
                        let (rx, ry) = (655350 / tablet_pad.0 as i32, 655350 / tablet_pad.1 as i32);
                        let (rx, ry) = (rx.max(1), ry.max(1));
                        match &mut sink {
                            Sink::Dry => println!("create tablet res={rx}x{ry}"),
                            Sink::Real { tablet, touch, .. } => {
                                *tablet = None; // destroy the old one first
                                *tablet = Some(input::tablet(rx, ry)?);
                                *touch = None;
                                *touch = Some(input::touch(rx, ry)?);
                            }
                        }
                        eprintln!(
                            "phone {from}: pad {:.1} x {:.1} mm",
                            tablet_pad.0 as f32 / 10.0,
                            tablet_pad.1 as f32 / 10.0
                        );
                    }
                }
                Reply::Stale(kind, seq) => {
                    if dry {
                        println!("drop {kind} seq={seq}");
                    }
                }
                Reply::Text { tack, tid, text } => {
                    let _ = sock.send_to(&tack, from);
                    if !dry && text.as_deref().is_some_and(|t| t.contains('\u{e025}')) {
                        // the phone asks which MAC to wake us with on this network (it differs per SSID)
                        let ws_seq = next_ws_seq();
                        let (out, kws, seq, session) = (sock.try_clone()?, kws.clone(), ws_seq, engine.session());
                        std::thread::spawn(move || {
                            if let Some(mac) = mac_towards(from.ip()) {
                                let _ = out.send_to(&proto::wslist(&kws, seq, session, &format!("mac\t{mac}\n")), from);
                            }
                        });
                    }
                    if !dry && text.as_deref().is_some_and(|t| t.contains('\u{e028}')) {
                        // the phone asks for the laptop's clipboard (it just came to the foreground)
                        let send = send_clip.clone();
                        std::thread::spawn(move || {
                            if let Some(t) = clip::current() {
                                send(t);
                            }
                        });
                    }
                    if !dry && text.as_deref().is_some_and(|t| t.contains('\u{e021}')) {
                        // the phone asked for the workspace list: hyprctl runs off the UDP loop
                        let ws_seq = next_ws_seq();
                        let (out, kws, seq, session) = (sock.try_clone()?, kws.clone(), ws_seq, engine.session());
                        std::thread::spawn(move || {
                            let _ = out.send_to(&proto::wslist(&kws, seq, session, &ws::list()), from);
                        });
                    }
                    match text {
                        Some(t) if dry => println!("text tid={tid} {}", text::describe(&t)),
                        Some(t) => typist.send(t)?,
                        None if dry => println!("dup TEXT tid={tid}"),
                        None => {}
                    }
                }
                Reply::Ignored => {}
            },
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e.into()),
        }

        if engine.active() && last_accept.elapsed() >= FAILSAFE {
            engine.release_all(&mut tf, &mut mf);
            eprintln!("failsafe: no STATE for 500 ms, released everything");
        }
        let touchf = std::mem::take(&mut engine.touchf);
        sink.frames(&tf, &mf, &touchf)?;
        #[cfg(target_os = "macos")]
        if !dry {
            use engine::{BTN_LEFT, BTN_TOUCH, EV_KEY};
            let released = mf
                .events()
                .iter()
                .any(|e| e.kind == EV_KEY && e.code == BTN_LEFT && e.value == 0)
                || tf
                    .events()
                    .iter()
                    .chain(touchf.events())
                    .any(|e| e.kind == EV_KEY && e.code == BTN_TOUCH && e.value == 0);
            let session = engine.session();
            if released && session != 0 {
                if let Some(to) = *phone_addr.lock().unwrap() {
                    let gen = ime_gen
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        .wrapping_add(1);
                    let (gen_ref, out, kws) = (ime_gen.clone(), ime_sock.clone(), kws.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(120));
                        if gen_ref.load(std::sync::atomic::Ordering::SeqCst) == gen
                            && macos::focused_is_text_input()
                        {
                            let _ = out.send_to(
                                &proto::wslist(&kws, next_ws_seq(), session, "ime\t1\n"),
                                to,
                            );
                        }
                    });
                }
            }
        }

        // Read timeout only while something is held; setsockopt only on transitions
        // or after a non-accepted wakeup, never per accepted packet.
        if engine.active() {
            if !(armed && accepted) {
                let left = FAILSAFE.saturating_sub(last_accept.elapsed());
                sock.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
                armed = true;
            }
        } else if armed {
            sock.set_read_timeout(None)?;
            armed = false;
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut port, mut sens, mut dry, mut host, mut do_pair) = (47800u16, 20f32, false, None, false);
    let mut alts = Vec::new();
    let mut screen = stream::Screen::Virtual;
    let mut scale = "1.5".to_string();
    let bad = |m: &str| -> ! {
        eprintln!("{m}\n\n{USAGE}");
        std::process::exit(2)
    };
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| bad(&format!("{a} needs a value")));
        match a.as_str() {
            "pair" => do_pair = true,
            "--dry-run" => dry = true,
            "--port" => port = val().parse().unwrap_or_else(|_| bad("bad --port")),
            "--sensitivity" => {
                sens = val().parse().ok().filter(|s: &f32| s.is_finite() && *s > 0.0).unwrap_or_else(|| bad("bad --sensitivity"))
            }
            "--host" => host = Some(val()),
            "--alt" => alts.push(val()),
            "--screen" => {
                screen = match val() {
                    v if v == "virtual" => stream::Screen::Virtual,
                    v if v == "focused" => stream::Screen::Focused,
                    #[cfg(target_os = "macos")]
                    v if v == "window" => stream::Screen::Window,
                    v => stream::Screen::Named(v),
                }
            }
            "--scale" => {
                scale = val();
                if !scale.parse::<f32>().is_ok_and(|s| (0.5..=4.0).contains(&s)) {
                    bad("bad --scale")
                }
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => bad(&format!("unknown argument: {a}")),
        }
    }
    let r = if do_pair { pair(port, host, alts) } else { run(port, sens, dry, screen, scale) };
    if let Err(e) = r {
        eprintln!("spenmouse-receiver: {e}");
        std::process::exit(1);
    }
}
