// Wire format, CONTRACT.md section 3. All integers big-endian.
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use hmac::{Hmac, Mac as _};
use sha2::Sha256;

pub type Mac = Hmac<Sha256>;

pub const HELLO: u8 = 1;
pub const WELCOME: u8 = 2;
pub const STATE: u8 = 3;
pub const TEXT: u8 = 4;
pub const TACK: u8 = 5;
pub const STREAM: u8 = 6;
pub const WSLIST: u8 = 7;
pub const TEXT_MAX: usize = 20 + 1024 + 16;

pub type Cipher = chacha20poly1305::ChaCha20Poly1305;

/// Section 8: Ktxt / Kvid = HMAC-SHA256(K, label).
pub fn derive(key: &[u8; 16], label: &str) -> [u8; 32] {
    let mut m = mac(key);
    m.update(label.as_bytes());
    m.finalize().into_bytes().into()
}

pub fn text_cipher(key: &[u8; 16]) -> Cipher {
    Cipher::new(&derive(key, "spenmouse text").into())
}

pub fn ws_cipher(key: &[u8; 16]) -> Cipher {
    Cipher::new(&derive(key, "spenmouse ws").into())
}

/// WSLIST (section 8): header + session + AEAD(text), nonce = session + seq, AAD = first 16 bytes.
pub fn wslist(c: &Cipher, seq: u32, session: u64, plain: &str) -> Vec<u8> {
    let mut p = vec![0x53, 0x4d, 1, WSLIST];
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&session.to_be_bytes());
    let ct = c.encrypt(&text_nonce(session, seq).into(), Payload { msg: plain.as_bytes(), aad: &p }).expect("wslist");
    p.extend_from_slice(&ct);
    p
}

pub fn video_cipher(key: &[u8; 16]) -> Cipher {
    Cipher::new(&derive(key, "spenmouse video").into())
}

#[derive(Debug, PartialEq)]
pub struct Text {
    pub seq: u32,
    pub session: u64,
    pub tid: u32,
    pub text: String,
}

fn text_nonce(session: u64, seq: u32) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&session.to_be_bytes());
    n[8..].copy_from_slice(&seq.to_be_bytes());
    n
}

/// TEXT is the only AEAD packet. None = drop silently.
pub fn parse_text(c: &Cipher, p: &[u8]) -> Option<Text> {
    if !(20 + 1 + 16..=TEXT_MAX).contains(&p.len()) || p[..4] != [0x53, 0x4d, 1, TEXT] {
        return None;
    }
    let seq = u32::from_be_bytes(be(p, 4));
    let session = u64::from_be_bytes(be(p, 8));
    let plain = c
        .decrypt(&text_nonce(session, seq).into(), Payload { msg: &p[20..], aad: &p[..20] })
        .ok()?;
    // authenticated but not UTF-8 is a phone bug: type what we can so it still gets its TACK
    let text = String::from_utf8_lossy(&plain).into_owned();
    Some(Text { seq, session, tid: u32::from_be_bytes(be(p, 16)), text })
}

/// One video record (section 9): len u32 + ciphertext, nonce = np + record counter.
pub fn record(c: &Cipher, np: [u8; 4], n: u64, plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(&np);
    nonce[4..].copy_from_slice(&n.to_be_bytes());
    let ct = c.encrypt(&nonce.into(), plain).expect("record too large");
    let mut out = (ct.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&ct);
    out
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct State {
    pub seq: u32,
    pub session: u64,
    pub flags: u8,
    pub x: u16,
    pub y: u16,
    pub pressure: u16,
    pub wheel: i16,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Packet {
    Hello { seq: u32, client: u32, pad_w: u16, pad_h: u16 },
    Welcome { seq: u32, session: u64 },
    State(State),
    Tack { seq: u32, session: u64, tid: u32 },
    Stream { seq: u32, session: u64, w: u16, h: u16 },
}

pub fn mac(key: &[u8; 16]) -> Mac {
    <Mac as hmac::Mac>::new_from_slice(key).unwrap()
}

fn be<const N: usize>(p: &[u8], at: usize) -> [u8; N] {
    p[at..at + N].try_into().unwrap()
}

/// None = drop silently (bad magic, version, length, tag or unknown type).
pub fn parse(mac: &Mac, p: &[u8]) -> Option<Packet> {
    if p.len() < 16 || p[0] != 0x53 || p[1] != 0x4d || p[2] != 1 {
        return None;
    }
    let want = match p[3] {
        HELLO | WELCOME => 24,
        STATE => 33,
        TACK | STREAM => 28,
        _ => return None,
    };
    if p.len() != want {
        return None;
    }
    let (body, tag) = p.split_at(want - 8);
    let mut m = mac.clone();
    m.update(body);
    m.verify_truncated_left(tag).ok()?; // constant-time
    let seq = u32::from_be_bytes(be(p, 4));
    Some(match p[3] {
        HELLO => Packet::Hello {
            seq,
            client: u32::from_be_bytes(be(p, 8)),
            pad_w: u16::from_be_bytes(be(p, 12)),
            pad_h: u16::from_be_bytes(be(p, 14)),
        },
        WELCOME => Packet::Welcome { seq, session: u64::from_be_bytes(be(p, 8)) },
        TACK => Packet::Tack {
            seq,
            session: u64::from_be_bytes(be(p, 8)),
            tid: u32::from_be_bytes(be(p, 16)),
        },
        STREAM => Packet::Stream {
            seq,
            session: u64::from_be_bytes(be(p, 8)),
            w: u16::from_be_bytes(be(p, 16)),
            h: u16::from_be_bytes(be(p, 18)),
        },
        _ => Packet::State(State {
            seq,
            session: u64::from_be_bytes(be(p, 8)),
            flags: p[16],
            x: u16::from_be_bytes(be(p, 17)),
            y: u16::from_be_bytes(be(p, 19)),
            pressure: u16::from_be_bytes(be(p, 21)),
            wheel: i16::from_be_bytes(be(p, 23)),
        }),
    })
}

fn seal<const N: usize>(mac: &Mac, kind: u8, seq: u32, payload: &[u8]) -> [u8; N] {
    let mut b = [0u8; N];
    b[..4].copy_from_slice(&[0x53, 0x4d, 1, kind]);
    b[4..8].copy_from_slice(&seq.to_be_bytes());
    b[8..N - 8].copy_from_slice(payload);
    let mut m = mac.clone();
    m.update(&b[..N - 8]);
    b[N - 8..].copy_from_slice(&m.finalize().into_bytes()[..8]);
    b
}

pub fn welcome(mac: &Mac, seq: u32, session: u64) -> [u8; 24] {
    seal(mac, WELCOME, seq, &session.to_be_bytes())
}

pub fn tack(mac: &Mac, seq: u32, session: u64, tid: u32) -> [u8; 28] {
    let mut p = [0u8; 12];
    p[..8].copy_from_slice(&session.to_be_bytes());
    p[8..].copy_from_slice(&tid.to_be_bytes());
    seal(mac, TACK, seq, &p)
}

#[cfg(test)]
pub fn stream(mac: &Mac, seq: u32, session: u64, w: u16, h: u16) -> [u8; 28] {
    let mut p = [0u8; 12];
    p[..8].copy_from_slice(&session.to_be_bytes());
    p[8..10].copy_from_slice(&w.to_be_bytes());
    p[10..].copy_from_slice(&h.to_be_bytes());
    seal(mac, STREAM, seq, &p)
}

#[cfg(test)]
pub fn text(c: &Cipher, seq: u32, session: u64, tid: u32, text: &str) -> Vec<u8> {
    let mut p = vec![0x53, 0x4d, 1, TEXT];
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&session.to_be_bytes());
    p.extend_from_slice(&tid.to_be_bytes());
    let ct = c
        .encrypt(&text_nonce(session, seq).into(), Payload { msg: text.as_bytes(), aad: &p })
        .unwrap();
    p.extend_from_slice(&ct);
    p
}

#[cfg(test)]
pub fn hello(mac: &Mac, seq: u32, client: u32, pad_w: u16, pad_h: u16) -> [u8; 24] {
    let mut p = [0u8; 8];
    p[..4].copy_from_slice(&client.to_be_bytes());
    p[4..6].copy_from_slice(&pad_w.to_be_bytes());
    p[6..].copy_from_slice(&pad_h.to_be_bytes());
    seal(mac, HELLO, seq, &p)
}

#[cfg(test)]
pub fn state(mac: &Mac, s: &State) -> [u8; 33] {
    let mut p = [0u8; 17];
    p[..8].copy_from_slice(&s.session.to_be_bytes());
    p[8] = s.flags;
    p[9..11].copy_from_slice(&s.x.to_be_bytes());
    p[11..13].copy_from_slice(&s.y.to_be_bytes());
    p[13..15].copy_from_slice(&s.pressure.to_be_bytes());
    p[15..].copy_from_slice(&s.wheel.to_be_bytes());
    seal(mac, STATE, s.seq, &p)
}

#[cfg(test)]
pub const TEST_KEY: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    const V_HELLO: &str = "534d0101000000010102030405dc02bc8d387c9b70989305";
    const V_WELCOME: &str = "534d0102000000011122334455667788214588e1a8d5a9ae";
    const V_STATE: &str = "534d010300000002112233445566778807800040009c40fffd152ce5b8b9bbe8d8";
    const ST: State = State {
        seq: 2,
        session: 0x1122334455667788,
        flags: 0x07,
        x: 32768,
        y: 16384,
        pressure: 40000,
        wheel: -3,
    };

    #[test]
    fn vectors_build() {
        let m = mac(&TEST_KEY);
        assert_eq!(hello(&m, 1, 0x01020304, 1500, 700).to_vec(), unhex(V_HELLO));
        assert_eq!(welcome(&m, 1, 0x1122334455667788).to_vec(), unhex(V_WELCOME));
        assert_eq!(state(&m, &ST).to_vec(), unhex(V_STATE));
    }

    #[test]
    fn vectors_parse() {
        let m = mac(&TEST_KEY);
        assert_eq!(
            parse(&m, &unhex(V_HELLO)),
            Some(Packet::Hello { seq: 1, client: 0x01020304, pad_w: 1500, pad_h: 700 })
        );
        assert_eq!(
            parse(&m, &unhex(V_WELCOME)),
            Some(Packet::Welcome { seq: 1, session: 0x1122334455667788 })
        );
        assert_eq!(parse(&m, &unhex(V_STATE)), Some(Packet::State(ST)));
    }

    const SES: u64 = 0x1122334455667788;
    const V_TEXT: &str = "534d010400000007112233445566778800000003bb59e0163ec12df08c99da7fd50363e129cec0ef926c38c839";
    const V_TACK: &str = "534d01050000000711223344556677880000000321252a0029bd1880";
    const V_STREAM: &str = "534d0106000000081122334455667788092404381863742be5c3fe33";

    #[test]
    fn v2_keys() {
        assert_eq!(
            derive(&TEST_KEY, "spenmouse text").to_vec(),
            unhex("fc70dc61da411f90026cfa60caef55ed81f96a531b26090fb28d900e3b530283")
        );
        assert_eq!(
            derive(&TEST_KEY, "spenmouse video").to_vec(),
            unhex("517c896910f417b1564da3aba8b57147ecaaa06e13f2e04f2c7c97881d29d66c")
        );
    }

    #[test]
    fn v2_text() {
        let c = text_cipher(&TEST_KEY);
        let s = "Hola \u{f1}\n\u{8}";
        assert_eq!(text(&c, 7, SES, 3, s), unhex(V_TEXT));
        let good = unhex(V_TEXT);
        assert_eq!(parse_text(&c, &good), Some(Text { seq: 7, session: SES, tid: 3, text: s.into() }));
        for i in 0..good.len() {
            let mut p = good.clone();
            p[i] ^= 1; // header is AAD (seq and session are also the nonce), the rest is ciphertext + tag
            assert_eq!(parse_text(&c, &p), None, "byte {i}");
        }
        assert_eq!(parse_text(&c, &good[..good.len() - 1]), None);
        assert_eq!(parse_text(&video_cipher(&TEST_KEY), &good), None);
        // length limits: n = 0 and n = 1025 are not TEXT
        assert_eq!(parse_text(&c, &text(&c, 7, SES, 3, "")), None);
        assert!(parse_text(&c, &text(&c, 7, SES, 3, &"a".repeat(1024))).is_some());
        assert_eq!(parse_text(&c, &text(&c, 7, SES, 3, &"a".repeat(1025))), None);
    }

    #[test]
    fn v2_tack_stream() {
        let m = mac(&TEST_KEY);
        assert_eq!(tack(&m, 7, SES, 3).to_vec(), unhex(V_TACK));
        assert_eq!(parse(&m, &unhex(V_TACK)), Some(Packet::Tack { seq: 7, session: SES, tid: 3 }));
        assert_eq!(stream(&m, 8, SES, 2340, 1080).to_vec(), unhex(V_STREAM));
        assert_eq!(
            parse(&m, &unhex(V_STREAM)),
            Some(Packet::Stream { seq: 8, session: SES, w: 2340, h: 1080 })
        );
        let good = unhex(V_STREAM);
        for i in 0..good.len() {
            let mut p = good.clone();
            p[i] ^= 1;
            assert_eq!(parse(&m, &p), None, "byte {i}");
        }
    }

    #[test]
    fn v2_records() {
        let c = video_cipher(&TEST_KEY);
        let np = [0xa1, 0xb2, 0xc3, 0xd4];
        assert_eq!(
            record(&c, np, 0, &unhex("000000016742")),
            unhex("00000016aa9c9b751ebe8610514b7071c1068211f45d08e62765")
        );
        assert_eq!(
            record(&c, np, 1, &unhex("0000000165")),
            unhex("000000151d2ab61769c1084f6e26b3b1fa590b79c17b148f66")
        );
    }

    #[test]
    fn bad_packets_dropped() {
        let m = mac(&TEST_KEY);
        let good = unhex(V_STATE);
        for i in 0..good.len() {
            let mut p = good.clone();
            p[i] ^= 1; // any flipped bit: magic, version, type, body or tag
            assert_eq!(parse(&m, &p), None, "byte {i}");
        }
        assert_eq!(parse(&m, &good[..32]), None);
        assert_eq!(parse(&m, &[good.clone(), vec![0]].concat()), None);
        assert_eq!(parse(&mac(&[9; 16]), &good), None);
        assert_eq!(parse(&m, &[]), None);
    }
}

#[cfg(test)]
mod ws_tests {
    use super::*;

    #[test]
    fn wslist_vector() {
        let p = wslist(&ws_cipher(&TEST_KEY), 9, 0x1122334455667788, "11\tHDMI-A-1\tkitty: vpn 1\n");
        let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "534d0107000000091122334455667788b3adba958de12e533cda79eb6a5b849b19c32119d93d01d26c0224c54d0c7673c8ada9d01af0016284");
    }
}
