// Session rules (CONTRACT.md section 5) and state -> input events (section 4).
// Pure logic: no sockets, no uinput, no allocation.
use crate::proto::{self, Cipher, Mac, Packet, State};

pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;
pub const ABS_PRESSURE: u16 = 0x18;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_WHEEL: u16 = 8;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_TOOL_PEN: u16 = 0x140;
pub const BTN_TOUCH: u16 = 0x14a;
pub const BTN_STYLUS: u16 = 0x14b;

const IN_RANGE: u8 = 1;
const CONTACT: u8 = 2;
const TOOL_PEN: u8 = 4;
const PEN_BUTTON: u8 = 0x40;
const PEN_ABS: u8 = 0x80;
// IN_RANGE or any button; CONTACT alone holds nothing down.
const ACTIVE: u8 = 0x79;

#[derive(Debug, PartialEq, Clone, Copy, Default)]
pub struct Ev {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// One evdev frame for one device, without the trailing SYN_REPORT.
#[derive(Default)]
pub struct Frame {
    pub n: usize,
    pub ev: [Ev; 6],
}

impl Frame {
    pub fn push(&mut self, kind: u16, code: u16, value: i32) {
        self.ev[self.n] = Ev { kind, code, value };
        self.n += 1;
    }
    pub fn events(&self) -> &[Ev] {
        &self.ev[..self.n]
    }
}

pub enum Reply {
    Ignored,
    Welcome([u8; 24]),
    Accepted,
    /// Valid packet of this kind with the wrong session or an old seq.
    Stale(&'static str, u32),
    /// Accepted TEXT: always TACK; `text` is None for an already applied tid.
    Text { tack: [u8; 28], tid: u32, text: Option<String> },
}

pub struct Engine {
    mac: Mac,
    ktxt: Cipher,
    last_tid: Option<u32>,
    sens: f32,
    client: u32,
    session: u64, // 0 = no session
    last_seq: Option<u32>,
    pub pad: (u16, u16),
    wheel_ref: Option<i16>,
    finger_ref: Option<(u16, u16)>,
    ref_pen: bool,
    rem: (f32, f32),
    last_flags: u8,
    // what the devices currently show
    x: i32,
    y: i32,
    pressure: i32,
    tool: bool,
    touch: bool,
    buttons: u8,
    /// Events for the touchscreen device produced by the last packet; the caller takes them.
    pub touchf: Frame,
    finger: (i32, i32, bool), // last touchscreen x, y, down
}

impl Engine {
    pub fn new(key: &[u8; 16], sens: f32) -> Self {
        Engine {
            mac: proto::mac(key),
            ktxt: proto::text_cipher(key),
            last_tid: None,
            sens,
            client: 0,
            session: 0,
            last_seq: None,
            pad: (0, 0),
            wheel_ref: None,
            finger_ref: None,
            ref_pen: false,
            rem: (0.0, 0.0),
            last_flags: 0,
            x: -1,
            y: -1,
            pressure: 0,
            tool: false,
            touch: false,
            buttons: 0,
            touchf: Frame::default(),
            finger: (-1, -1, false),
        }
    }

    pub fn active(&self) -> bool {
        self.last_flags & ACTIVE != 0
    }

    /// Session and seq rule shared by STATE, TEXT and STREAM (one last_seq for all).
    pub fn session(&self) -> u64 {
        self.session
    }

    fn fresh(&mut self, session: u64, seq: u32) -> bool {
        if self.session == 0 || session != self.session || self.last_seq.is_some_and(|l| seq <= l) {
            return false;
        }
        self.last_seq = Some(seq);
        true
    }

    /// STREAM request read from TCP (section 9). Some((w, h)) = start the video.
    pub fn on_stream(&mut self, p: &[u8]) -> Option<(u16, u16)> {
        match proto::parse(&self.mac, p) {
            Some(Packet::Stream { seq, session, w, h }) if self.fresh(session, seq) => Some((w, h)),
            _ => None,
        }
    }

    pub fn on_packet(
        &mut self,
        p: &[u8],
        new_session: &mut dyn FnMut() -> u64,
        tablet: &mut Frame,
        mouse: &mut Frame,
    ) -> Reply {
        if p.get(3) == Some(&proto::TEXT) {
            let Some(t) = proto::parse_text(&self.ktxt, p) else { return Reply::Ignored };
            if !self.fresh(t.session, t.seq) {
                return Reply::Stale("TEXT", t.seq);
            }
            let new = self.last_tid.is_none_or(|l| t.tid > l);
            if new {
                self.last_tid = Some(t.tid);
            }
            return Reply::Text {
                tack: proto::tack(&self.mac, t.seq, t.session, t.tid),
                tid: t.tid,
                text: new.then_some(t.text),
            };
        }
        match proto::parse(&self.mac, p) {
            Some(Packet::Hello { seq, client, pad_w, pad_h }) => {
                if self.session == 0 || client != self.client {
                    self.session = new_session().max(1);
                    self.client = client;
                    self.last_seq = None;
                    self.last_tid = None;
                    self.wheel_ref = None;
                    self.finger_ref = None;
                }
                self.pad = (pad_w.max(1), pad_h.max(1));
                Reply::Welcome(proto::welcome(&self.mac, seq, self.session))
            }
            Some(Packet::State(s)) => {
                if !self.fresh(s.session, s.seq) {
                    return Reply::Stale("STATE", s.seq);
                }
                self.last_flags = s.flags;
                self.apply(&s, true, tablet, mouse);
                Reply::Accepted
            }
            _ => Reply::Ignored,
        }
    }

    /// 500 ms failsafe: release every button and take the pen out of range.
    pub fn release_all(&mut self, tablet: &mut Frame, mouse: &mut Frame) {
        self.last_flags = 0;
        let s = State { seq: 0, session: 0, flags: 0, x: 0, y: 0, pressure: 0, wheel: 0 };
        self.apply(&s, false, tablet, mouse);
    }

    fn apply(&mut self, s: &State, wheel: bool, tablet: &mut Frame, mouse: &mut Frame) {
        let f = s.flags;
        let pen = f & TOOL_PEN != 0;
        let tool = pen && f & PEN_ABS != 0 && f & IN_RANGE != 0;
        let touch = tool && f & CONTACT != 0;
        let stylus = pen && f & PEN_BUTTON != 0;

        // Mobile mode: a finger with PEN_ABS is a touchscreen touch at x,y (position first, then BTN_TOUCH).
        let finger = !pen && f & PEN_ABS != 0 && f & CONTACT != 0;
        if finger {
            if self.finger.0 != s.x as i32 {
                self.finger.0 = s.x as i32;
                self.touchf.push(EV_ABS, ABS_X, s.x as i32);
            }
            if self.finger.1 != s.y as i32 {
                self.finger.1 = s.y as i32;
                self.touchf.push(EV_ABS, ABS_Y, s.y as i32);
            }
        }
        if finger != self.finger.2 {
            self.finger.2 = finger;
            self.touchf.push(EV_KEY, BTN_TOUCH, finger as i32);
        }

        if tool {
            for (code, cur, new) in [
                (ABS_X, &mut self.x, s.x as i32),
                (ABS_Y, &mut self.y, s.y as i32),
                (ABS_PRESSURE, &mut self.pressure, s.pressure as i32),
            ] {
                if *cur != new {
                    *cur = new;
                    tablet.push(EV_ABS, code, new);
                }
            }
        }
        for (code, cur, new) in [
            (BTN_TOOL_PEN, &mut self.tool, tool),
            (BTN_TOUCH, &mut self.touch, touch),
        ] {
            if *cur != new {
                *cur = new;
                tablet.push(EV_KEY, code, new as i32);
            }
        }

        // Hyprland no convierte BTN_STYLUS en clic: el boton lateral sale como clic derecho del mouse.
        let buttons = ((f >> 3) & 7) | (stylus as u8) << 1;
        for (bit, code) in [(1, BTN_LEFT), (2, BTN_RIGHT), (4, BTN_MIDDLE)] {
            if (buttons ^ self.buttons) & bit != 0 {
                mouse.push(EV_KEY, code, (buttons & bit != 0) as i32);
            }
        }
        self.buttons = buttons;

        // relativo: dedo en contacto, o lapiz sin PEN_ABS en rango (flotando o tocando)
        let moving = f & PEN_ABS == 0 && f & if pen { IN_RANGE } else { CONTACT } != 0;
        if moving {
            if let Some((rx, ry)) = self.finger_ref.filter(|_| self.ref_pen == pen) {
                // x,y span the pad; pad is in tenths of mm -> mm -> counts
                let k = self.sens / 655350.0;
                self.rem.0 += (s.x as i32 - rx as i32) as f32 * self.pad.0 as f32 * k;
                self.rem.1 += (s.y as i32 - ry as i32) as f32 * self.pad.1 as f32 * k;
                let (dx, dy) = (self.rem.0.trunc(), self.rem.1.trunc());
                self.rem.0 -= dx;
                self.rem.1 -= dy;
                if dx != 0.0 {
                    mouse.push(EV_REL, REL_X, dx as i32);
                }
                if dy != 0.0 {
                    mouse.push(EV_REL, REL_Y, dy as i32);
                }
            }
            self.finger_ref = Some((s.x, s.y));
            self.ref_pen = pen;
        } else {
            self.finger_ref = None;
            self.rem = (0.0, 0.0);
        }

        if wheel {
            if let Some(r) = self.wheel_ref {
                let d = s.wheel.wrapping_sub(r);
                if d != 0 {
                    mouse.push(EV_REL, REL_WHEEL, d as i32);
                }
            }
            self.wheel_ref = Some(s.wheel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::TEST_KEY;

    struct T {
        e: Engine,
        mac: Mac,
        session: u64,
        next: u64,
    }

    type Evs = Vec<(u16, u16, i32)>;

    impl T {
        fn new() -> T {
            T { e: Engine::new(&TEST_KEY, 10.0), mac: proto::mac(&TEST_KEY), session: 0, next: 100 }
        }
        fn send(&mut self, p: &[u8]) -> (Reply, Evs, Evs) {
            let (mut t, mut m) = (Frame::default(), Frame::default());
            let next = &mut self.next;
            let r = self.e.on_packet(
                p,
                &mut || {
                    *next += 1;
                    *next
                },
                &mut t,
                &mut m,
            );
            let v = |f: &Frame| f.events().iter().map(|e| (e.kind, e.code, e.value)).collect();
            (r, v(&t), v(&m))
        }
        fn hello(&mut self, seq: u32, client: u32) -> u64 {
            let p = proto::hello(&self.mac, seq, client, 1000, 500);
            match self.send(&p).0 {
                Reply::Welcome(w) => match proto::parse(&self.mac, &w) {
                    Some(Packet::Welcome { seq: s, session }) => {
                        assert_eq!(s, seq);
                        self.session = session;
                        session
                    }
                    _ => panic!("bad welcome"),
                },
                _ => panic!("no welcome"),
            }
        }
        fn st(&mut self, seq: u32, flags: u8, x: u16, y: u16, wheel: i16) -> Option<(Evs, Evs)> {
            let s = State { seq, session: self.session, flags, x, y, pressure: 0, wheel };
            let p = proto::state(&self.mac, &s);
            match self.send(&p) {
                (Reply::Accepted, t, m) => Some((t, m)),
                (Reply::Stale(..), t, m) => {
                    assert!(t.is_empty() && m.is_empty());
                    None
                }
                _ => panic!("ignored"),
            }
        }
    }

    #[test]
    fn session_rules() {
        let mut t = T::new();
        // no session yet: STATE rejected
        assert!(t.st(1, 0, 0, 0, 0).is_none());
        let s1 = t.hello(10, 0xAA);
        assert_ne!(s1, 0);
        // same client: same session, last_seq kept
        assert!(t.st(11, 0, 0, 0, 0).is_some());
        assert_eq!(t.hello(12, 0xAA), s1);
        assert!(t.st(11, 0, 0, 0, 0).is_none(), "duplicate");
        assert!(t.st(5, 0, 0, 0, 0).is_none(), "out of order");
        assert!(t.st(13, 0, 0, 0, 0).is_some());
        // wrong session
        t.session = s1 ^ 1;
        assert!(t.st(14, 0, 0, 0, 0).is_none());
        // other client: new session, last_seq forgotten, old session dead
        let s2 = t.hello(1, 0xBB);
        assert_ne!(s2, s1);
        assert!(t.st(2, 0, 0, 0, 0).is_some(), "seq below old last_seq is fine in new session");
        t.session = s1;
        assert!(t.st(1000, 0, 0, 0, 0).is_none(), "old session replay");
    }

    impl T {
        /// Some(Some(text)) applied, Some(None) duplicate (TACK only), None not accepted.
        fn text(&mut self, seq: u32, tid: u32, s: &str) -> Option<Option<String>> {
            let p = proto::text(&proto::text_cipher(&TEST_KEY), seq, self.session, tid, s);
            match self.send(&p).0 {
                Reply::Text { tack, tid: t, text } => {
                    assert_eq!(t, tid);
                    assert_eq!(tack, proto::tack(&self.mac, seq, self.session, tid));
                    Some(text)
                }
                Reply::Stale("TEXT", s) => {
                    assert_eq!(s, seq);
                    None
                }
                _ => panic!("ignored"),
            }
        }
    }

    #[test]
    fn text_tid_rules() {
        let mut t = T::new();
        assert_eq!(t.text(1, 1, "x"), None, "no session");
        t.hello(10, 0xAA);
        // first tid of a session is applied whatever its value
        assert_eq!(t.text(11, 500, "a"), Some(Some("a".into())));
        // retransmit: new seq, same tid -> TACK again, not applied again
        assert_eq!(t.text(12, 500, "a"), Some(None));
        assert_eq!(t.text(13, 499, "old"), Some(None), "lower tid");
        assert_eq!(t.text(14, 501, "b"), Some(Some("b".into())));
        // same datagram replayed (seq not above last_seq): no TACK at all
        assert_eq!(t.text(14, 502, "c"), None);
        // TEXT and STATE share last_seq
        assert!(t.st(14, 0, 0, 0, 0).is_none());
        assert!(t.st(15, 0, 0, 0, 0).is_some());
        assert_eq!(t.text(15, 502, "c"), None);
        assert_eq!(t.text(16, 502, "c"), Some(Some("c".into())));
        // wrong session
        let s = t.session;
        t.session = s ^ 1;
        assert_eq!(t.text(17, 503, "d"), None);
        // new session: tid starts over
        t.hello(1, 0xBB);
        assert_eq!(t.text(2, 1, "e"), Some(Some("e".into())));
    }

    #[test]
    fn tampered_text_is_ignored() {
        let mut t = T::new();
        t.hello(1, 1);
        let mut p = proto::text(&proto::text_cipher(&TEST_KEY), 2, t.session, 1, "hola");
        let n = p.len();
        p[n - 1] ^= 1;
        assert!(matches!(t.send(&p).0, Reply::Ignored));
        p[n - 1] ^= 1;
        p[19] ^= 1; // tid is AAD
        assert!(matches!(t.send(&p).0, Reply::Ignored));
        p[19] ^= 1;
        // the untampered packet is still fresh: nothing above consumed its seq or tid
        assert!(matches!(t.send(&p).0, Reply::Text { text: Some(_), .. }));
    }

    #[test]
    fn stream_request_rules() {
        let mut t = T::new();
        let m = t.mac.clone();
        assert_eq!(t.e.on_stream(&proto::stream(&m, 1, 0, 100, 100)), None, "no session");
        let s = t.hello(10, 1);
        let mut bad = proto::stream(&m, 11, s, 2340, 1080);
        bad[17] ^= 1; // tampered w
        assert_eq!(t.e.on_stream(&bad), None);
        assert_eq!(t.e.on_stream(&proto::stream(&m, 11, s ^ 1, 2340, 1080)), None, "wrong session");
        assert_eq!(t.e.on_stream(&proto::stream(&m, 11, s, 2340, 1080)), Some((2340, 1080)));
        assert_eq!(t.e.on_stream(&proto::stream(&m, 11, s, 2340, 1080)), None, "replay");
        // shared last_seq
        assert!(t.st(11, 0, 0, 0, 0).is_none());
        assert!(t.st(12, 0, 0, 0, 0).is_some());
        assert_eq!(t.e.on_stream(&proto::stream(&m, 12, s, 1, 1)), None);
        // only STREAM is a stream request
        t.session = s;
        assert_eq!(t.e.on_stream(&proto::state(&m, &State { seq: 99, session: s, flags: 0, x: 0, y: 0, pressure: 0, wheel: 0 })), None);
    }

    #[test]
    fn seq_compare_has_no_wraparound() {
        let mut t = T::new();
        t.hello(u32::MAX - 1, 1);
        assert!(t.st(u32::MAX, 0, 0, 0, 0).is_some());
        assert!(t.st(0, 0, 0, 0, 0).is_none());
    }

    #[test]
    fn wheel_reference_and_wraparound() {
        let mut t = T::new();
        t.hello(1, 1);
        // first STATE of the session only sets the reference
        assert_eq!(t.st(2, 0, 0, 0, 32766).unwrap().1, vec![]);
        assert_eq!(t.st(3, 0, 0, 0, 32767).unwrap().1, vec![(EV_REL, REL_WHEEL, 1)]);
        assert_eq!(t.st(4, 0, 0, 0, -32768).unwrap().1, vec![(EV_REL, REL_WHEEL, 1)]);
        assert_eq!(t.st(5, 0, 0, 0, 32765).unwrap().1, vec![(EV_REL, REL_WHEEL, -3)]);
        // a rejected packet must not move the reference
        assert!(t.st(5, 0, 0, 0, 0).is_none());
        assert_eq!(t.st(6, 0, 0, 0, 32765).unwrap().1, vec![]);
        // new session: reference reset
        t.hello(1, 2);
        assert_eq!(t.st(2, 0, 0, 0, 5).unwrap().1, vec![]);
    }

    #[test]
    fn pen_and_buttons_and_failsafe() {
        let mut t = T::new();
        t.hello(1, 1);
        let (tab, m) = t.st(2, 0x85, 100, 200, 0).unwrap();
        assert_eq!(tab, vec![(EV_ABS, ABS_X, 100), (EV_ABS, ABS_Y, 200), (EV_KEY, BTN_TOOL_PEN, 1)]);
        assert!(m.is_empty());
        let (tab, m) = t.st(3, 0xcf, 100, 200, 0).unwrap();
        assert_eq!(tab, vec![(EV_KEY, BTN_TOUCH, 1)]);
        assert_eq!(m, vec![(EV_KEY, BTN_LEFT, 1), (EV_KEY, BTN_RIGHT, 1)]);
        assert!(t.e.active());
        let (mut tab, mut m) = (Frame::default(), Frame::default());
        t.e.release_all(&mut tab, &mut m);
        assert_eq!(tab.n, 2);
        assert_eq!(
            m.events(),
            &[Ev { kind: EV_KEY, code: BTN_LEFT, value: 0 }, Ev { kind: EV_KEY, code: BTN_RIGHT, value: 0 }]
        );
        assert!(!t.e.active());
        // heartbeat resumes: absolute state is re-applied
        assert_eq!(t.st(4, 0xcf, 100, 200, 0).unwrap().1, vec![(EV_KEY, BTN_LEFT, 1), (EV_KEY, BTN_RIGHT, 1)]);
    }

    #[test]
    fn finger_absolute_is_a_touchscreen() {
        let mut t = T::new();
        t.hello(1, 1);
        // finger + PEN_ABS + CONTACT: touch down at x,y, nothing on mouse or tablet
        let (tab, m) = t.st(2, 0x83, 1000, 2000, 0).unwrap();
        assert!(tab.is_empty() && m.is_empty());
        assert_eq!(std::mem::take(&mut t.e.touchf).events(), &[
            Ev { kind: EV_ABS, code: ABS_X, value: 1000 },
            Ev { kind: EV_ABS, code: ABS_Y, value: 2000 },
            Ev { kind: EV_KEY, code: BTN_TOUCH, value: 1 },
        ]);
        let (_, m) = t.st(3, 0x83, 1500, 2000, 0).unwrap();
        assert!(m.is_empty()); // a drag is not relative mouse motion
        assert_eq!(std::mem::take(&mut t.e.touchf).events(), &[Ev { kind: EV_ABS, code: ABS_X, value: 1500 }]);
        t.st(4, 0x00, 0, 0, 0).unwrap();
        assert_eq!(std::mem::take(&mut t.e.touchf).events(), &[Ev { kind: EV_KEY, code: BTN_TOUCH, value: 0 }]);
    }

    #[test]
    fn pen_relative() {
        let mut t = T::new();
        t.hello(1, 1);
        // hover sin PEN_ABS: primer STATE fija referencia, el segundo mueve el mouse, nada en la tableta
        let (tab, m) = t.st(2, 0x05, 1000, 1000, 0).unwrap();
        assert!(tab.is_empty() && m.is_empty());
        let (tab, m) = t.st(3, 0x05, 1000 + 6554, 1000, 0).unwrap();
        assert!(tab.is_empty());
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].0, m[0].1), (EV_REL, REL_X));
        assert!(m[0].2 > 0);
        // cambia a dedo: no hereda la referencia del lapiz
        assert!(t.st(4, 0x03, 60000, 60000, 0).unwrap().1.is_empty());
    }

    #[test]
    fn finger_relative() {
        let mut t = T::new(); // pad 100.0 x 50.0 mm, 10 counts/mm
        t.hello(1, 1);
        assert_eq!(t.st(2, 0x03, 1000, 1000, 0).unwrap().1, vec![], "first contact = reference");
        // 6554/65535 of 100 mm = 100 counts; 59000/65535 of 50 mm = 450 counts
        assert_eq!(
            t.st(3, 0x03, 7554, 60000, 0).unwrap().1,
            vec![(EV_REL, REL_X, 100), (EV_REL, REL_Y, 450)]
        );
        assert_eq!(t.st(4, 0x00, 0, 0, 0).unwrap().1, vec![], "lift");
        assert_eq!(t.st(5, 0x03, 50000, 50000, 0).unwrap().1, vec![], "new contact does not jump");
        // pen in between also breaks the reference; finger never touches the tablet
        let (tab, m) = t.st(6, 0x87, 1, 1, 0).unwrap();
        assert!(!tab.is_empty() && m.is_empty());
        let (tab, m) = t.st(7, 0x03, 9, 9, 0).unwrap();
        assert_eq!(tab, vec![(EV_KEY, BTN_TOOL_PEN, 0), (EV_KEY, BTN_TOUCH, 0)]);
        assert!(m.is_empty());
    }
}
