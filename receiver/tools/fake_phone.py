#!/usr/bin/env python3
"""Fake phone for spenmouse-receiver (CONTRACT.md v2). Stdlib + `cryptography`.

Sends HELLO, waits for WELCOME, then a scripted STATE and TEXT sequence.
stdout = the exact lines `spenmouse-receiver --dry-run` should print, so:

    spenmouse-receiver --dry-run > recv.out &
    tools/fake_phone.py > want.out && diff want.out recv.out

Assumes a freshly started receiver with the default --sensitivity 20.

Video instead of the script (section 9), H.264 written to FILE:

    tools/fake_phone.py --stream out.h264 --seconds 3 [--size 2340x1080] [--replace]

--replace opens a second STREAM while the first is running and checks the first gets closed.
"""
import argparse, hashlib, hmac, json, os, socket, struct, sys, time
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305

ap = argparse.ArgumentParser()
ap.add_argument("--host", default="127.0.0.1")
ap.add_argument("--port", type=int, default=47800)
ap.add_argument("--key", help="32 hex (default: ~/.config/spenmouse/key)")
ap.add_argument("--sensitivity", type=float, default=20.0)
ap.add_argument("--stream", metavar="FILE", help="request the video and write the H.264 here")
ap.add_argument("--seconds", type=float, default=3.0)
ap.add_argument("--size", default="2340x1080", help="phone screen WxH sent in STREAM")
ap.add_argument("--replace", action="store_true")
a = ap.parse_args()
K = bytes.fromhex(a.key or open(os.path.expanduser("~/.config/spenmouse/key")).read().strip())

PAD_W, PAD_H = 1500, 700


def seal(key, body):
    return body + hmac.new(key, body, hashlib.sha256).digest()[:8]


def hello(key, seq, client, w, h):
    return seal(key, struct.pack(">2sBBIIHH", b"SM", 1, 1, seq, client, w, h))


def welcome(key, seq, session):
    return seal(key, struct.pack(">2sBBIQ", b"SM", 1, 2, seq, session))


def state(key, seq, session, flags, x, y, pressure, wheel):
    return seal(key, struct.pack(">2sBBIQBHHHh", b"SM", 1, 3, seq, session, flags, x, y, pressure, wheel))


def derive(key, label):
    return hmac.new(key, label, hashlib.sha256).digest()


def text(key, seq, session, tid, s):
    head = struct.pack(">2sBBIQI", b"SM", 1, 4, seq, session, tid)
    nonce = struct.pack(">QI", session, seq)
    return head + ChaCha20Poly1305(derive(key, b"spenmouse text")).encrypt(nonce, s.encode(), head)


def tack(key, seq, session, tid):
    return seal(key, struct.pack(">2sBBIQI", b"SM", 1, 5, seq, session, tid))


def stream(key, seq, session, w, h):
    return seal(key, struct.pack(">2sBBIQHH", b"SM", 1, 6, seq, session, w, h))


def record(key, np, n, plain):
    ct = ChaCha20Poly1305(derive(key, b"spenmouse video")).encrypt(np + struct.pack(">Q", n), plain, None)
    return struct.pack(">I", len(ct)) + ct


# self-check: contract sections 6 and 10
TK = bytes(range(16))
assert hello(TK, 1, 0x01020304, 1500, 700).hex() == "534d0101000000010102030405dc02bc8d387c9b70989305"
assert welcome(TK, 1, 0x1122334455667788).hex() == "534d0102000000011122334455667788214588e1a8d5a9ae"
assert state(TK, 2, 0x1122334455667788, 7, 32768, 16384, 40000, -3).hex() == \
    "534d010300000002112233445566778807800040009c40fffd152ce5b8b9bbe8d8"
TS = 0x1122334455667788
assert derive(TK, b"spenmouse text").hex() == "fc70dc61da411f90026cfa60caef55ed81f96a531b26090fb28d900e3b530283"
assert derive(TK, b"spenmouse video").hex() == "517c896910f417b1564da3aba8b57147ecaaa06e13f2e04f2c7c97881d29d66c"
assert text(TK, 7, TS, 3, "Hola \u00f1\n\b").hex() == \
    "534d010400000007112233445566778800000003bb59e0163ec12df08c99da7fd50363e129cec0ef926c38c839"
assert tack(TK, 7, TS, 3).hex() == "534d01050000000711223344556677880000000321252a0029bd1880"
assert stream(TK, 8, TS, 2340, 1080).hex() == "534d0106000000081122334455667788092404381863742be5c3fe33"
NP = bytes.fromhex("a1b2c3d4")
assert record(TK, NP, 0, bytes.fromhex("000000016742")).hex() == "00000016aa9c9b751ebe8610514b7071c1068211f45d08e62765"
assert record(TK, NP, 1, bytes.fromhex("0000000165")).hex() == "000000151d2ab61769c1084f6e26b3b1fa590b79c17b148f66"

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.settimeout(3)
sock.connect((a.host, a.port))
seq = 1000


def note(msg):
    print("#", msg, file=sys.stderr)


def want(*lines):
    for line in lines:
        print(line, flush=True)


def do_hello(client):
    global seq
    seq += 1
    sock.send(hello(K, seq, client, PAD_W, PAD_H))
    try:
        p = sock.recv(64)
    except OSError as e:
        sys.exit(f"no WELCOME from {a.host}:{a.port} ({e}): receiver not running, wrong key or firewall")
    assert len(p) == 24 and p == seal(K, p[:16]), "bad WELCOME tag/length"
    magic, ver, typ, wseq, session = struct.unpack(">2sBBIQ", p[:16])
    assert (magic, ver, typ, wseq) == (b"SM", 1, 2, seq), "bad WELCOME header/seq echo"
    assert session != 0
    return session


def send(flags, x=0, y=0, pressure=0, wheel=0, session=None, at=None, key=None):
    """Send one STATE. `at` forces a seq without advancing the counter."""
    global seq
    if at is None:
        seq += 1
    pkt = state(key or K, at or seq, session or S, flags, x, y, pressure, wheel)
    sock.send(pkt)
    time.sleep(0.005)
    return pkt


def read_exact(c, n):
    b = b""
    while len(b) < n:
        part = c.recv(n - len(b))
        if not part:
            return None
        b += part
    return b


def open_stream(pkt):
    c = socket.create_connection((a.host, a.port), timeout=5)
    c.sendall(pkt)
    return c, time.time()


def read_stream(c, t0, seconds, out):
    """Decrypt records for `seconds`; returns (bytes, records, s to first bytes, s to first keyframe complete)."""
    aead = ChaCha20Poly1305(derive(K, b"spenmouse video"))
    np = read_exact(c, 4)
    assert np is not None, "closed without np: STREAM rejected"
    n = total = 0
    first = full = None
    head = b""
    while time.time() - t0 < seconds:
        h = read_exact(c, 4)
        if h is None:
            break
        (ln,) = struct.unpack(">I", h)
        assert 16 < ln <= 1 << 20, f"bad record length {ln}"
        plain = aead.decrypt(np + struct.pack(">Q", n), read_exact(c, ln), None)
        first = first or time.time() - t0
        if full is None:  # first keyframe is complete once the next non-IDR slice starts
            head += plain
            if b"\x00\x00\x01\x21" in head or b"\x00\x00\x01\x41" in head or b"\x00\x00\x01\x01" in head:
                full = time.time() - t0
        out.write(plain)
        n += 1
        total += len(plain)
    return total, n, first, full


def do_stream():
    global seq
    w, h = map(int, a.size.split("x"))
    S = do_hello(int.from_bytes(os.urandom(4), "big"))  # new client = new session: seq may restart
    note("tampered STREAM and wrong session: closed without a byte")
    seq += 1
    bad = bytearray(stream(K, seq, S, w, h))
    bad[17] ^= 1
    for pkt in (bytes(bad), stream(K, seq, S ^ 1, w, h)):
        c, _ = open_stream(pkt)
        assert c.recv(1) == b"", "rejected STREAM got an answer"
        c.close()
    seq += 1
    first_c, t0 = open_stream(stream(K, seq, S, w, h))
    with open(a.stream, "wb") as out:
        if a.replace:
            note("first STREAM for 1 s, then a second one replaces it")
            with open(os.devnull, "wb") as null:
                read_stream(first_c, t0, 1.0, null)
            seq += 1
            c, t0 = open_stream(stream(K, seq, S, w, h))
            stats = read_stream(c, t0, a.seconds, out)
            first_c.settimeout(3)
            try:
                while first_c.recv(1 << 16):
                    pass
            except ConnectionError:
                pass  # reset also counts as closed
            note("first connection was closed by the receiver")
            first_c.close()
        else:
            c, t0 = first_c, t0
            stats = read_stream(c, t0, a.seconds, out)
    c.close()
    total, n, first, full = stats
    assert total, "no video"
    print(f"stream: {total} bytes in {n} records; first bytes {first:.3f} s, "
          f"first keyframe complete {full:.3f} s after connect -> {a.stream}")


if a.stream:
    do_stream()
    sys.exit()


def send_text(tid, s, expect_tack=True, pkt=None):
    """One TEXT (new seq, freshly encrypted unless `pkt`), then check the TACK."""
    global seq
    if pkt is None:
        seq += 1
        pkt = text(K, seq, S, tid, s)
    sock.send(pkt)
    pseq = struct.unpack(">I", pkt[4:8])[0]
    sock.settimeout(0.5)
    try:
        r = sock.recv(64)
        assert expect_tack, "TACK for a TEXT that must be dropped"
        assert r == tack(K, pseq, S, tid), "bad TACK"
    except socket.timeout:
        assert not expect_tack, f"no TACK for tid={tid}"
    return pkt


def typed(s):
    """Same form as the receiver's dry run: quoted text runs and key names, in order."""
    out, run = [], ""
    for ch in s:
        key = {"\b": "BackSpace", "\n": "Return", "\t": "Tab"}.get(ch)
        if key is None:
            run += ch
            continue
        if run:
            out.append(json.dumps(run, ensure_ascii=False))
            run = ""
        out.append(key)
    if run:
        out.append(json.dumps(run, ensure_ascii=False))
    return " ".join(out)


IN, CONTACT, PEN, LEFT, RIGHT, MIDDLE, PENBTN = 1, 2, 4 | 128, 8, 16, 32, 64  # PEN incluye PEN_ABS
W0 = 32766

note("HELLO -> WELCOME")
S = do_hello(0xC0FFEE01)
want(f"create tablet res={655350 // PAD_W}x{655350 // PAD_H}")

note("pen hover, contact, side button (first STATE only sets the wheel reference)")
send(IN | PEN, 10000, 20000, 0, W0)
want("tablet ABS_X=10000 ABS_Y=20000 BTN_TOOL_PEN=1")
contact = send(IN | CONTACT | PEN, 10100, 20000, 30000, W0)
contact_seq = seq
want("tablet ABS_X=10100 ABS_PRESSURE=30000 BTN_TOUCH=1")
send(IN | CONTACT | PEN | PENBTN, 10100, 20000, 30000, W0)
want("mouse BTN_RIGHT=1")

note("replay of an old datagram, out-of-order seq, wrong session, bad tag: all rejected")
sock.send(contact)
want(f"drop STATE seq={contact_seq}")
send(RIGHT, wheel=W0 + 1, at=seq - 2)
want(f"drop STATE seq={seq - 2}")
send(RIGHT, wheel=W0, session=S ^ 1)
want(f"drop STATE seq={seq}")
send(RIGHT, wheel=W0, key=bytes(16))  # wrong key = bad tag: silent

note("pen leaves")
send(0, wheel=W0)
want("tablet BTN_TOOL_PEN=0 BTN_TOUCH=0", "mouse BTN_RIGHT=0")

note("HELLO again, same client: same session")
assert do_hello(0xC0FFEE01) == S, "session changed for the same client"

note("finger: first contact is only a reference, then relative movement")
send(IN | CONTACT, 30000, 30000, 0, W0)
fx, fy = 32185, 29000
send(IN | CONTACT, fx, fy, 0, W0)
dx = int((fx - 30000) * PAD_W / 655350 * a.sensitivity)
dy = int((fy - 30000) * PAD_H / 655350 * a.sensitivity)
want(f"mouse REL_X={dx} REL_Y={dy}")

note("buttons and wheel (with i16 wraparound)")
send(IN | CONTACT | LEFT, fx, fy, 0, W0)
want("mouse BTN_LEFT=1")
send(IN | CONTACT, fx, fy, 0, W0)
want("mouse BTN_LEFT=0")
send(IN | CONTACT, fx, fy, 0, 32767)
want("mouse REL_WHEEL=1")
send(IN | CONTACT, fx, fy, 0, -32768)
want("mouse REL_WHEEL=1")
send(IN | CONTACT, fx, fy, 0, 32765)
want("mouse REL_WHEEL=-3")
send(RIGHT | MIDDLE, wheel=32765)
want("mouse BTN_RIGHT=1 BTN_MIDDLE=1")
send(0, wheel=32765)
want("mouse BTN_RIGHT=0 BTN_MIDDLE=0")

note("finger lands somewhere else: no jump")
send(IN | CONTACT, 60000, 5000, 0, 32765)
send(0, wheel=32765)

note("failsafe: pen down + left button held, then 0.8 s of silence")
send(IN | CONTACT | PEN | PENBTN | LEFT, 40000, 40000, 50000, 32765)
want("tablet ABS_X=40000 ABS_Y=40000 ABS_PRESSURE=50000 BTN_TOOL_PEN=1 BTN_TOUCH=1",
     "mouse BTN_LEFT=1 BTN_RIGHT=1")
time.sleep(0.8)
want("tablet BTN_TOOL_PEN=0 BTN_TOUCH=0", "mouse BTN_LEFT=0 BTN_RIGHT=0")

note("final idle state x3: nothing left to release")
for _ in range(3):
    send(0, wheel=32765)

note("another client takes over: new session, old session is dead")
old = S
S = do_hello(0xC0FFEE02)
assert S != old, "new client got the old session"
send(LEFT, session=old)
want(f"drop STATE seq={seq}")
send(0, wheel=5)   # first STATE of the new session: wheel reference only
send(0, wheel=4)
want("mouse REL_WHEEL=-1")
note("TEXT: first tid of the session is applied whatever its value; TACK every time")
t1 = "Hola \u00f1\n\b"
send_text(40, t1)
want(f"text tid=40 {typed(t1)}")
note("retransmits of the same tid (new seq, encrypted again): TACK, not typed again")
for _ in range(2):
    send_text(40, t1)
    want("dup TEXT tid=40")
note("text that looks like an option, keys in the middle")
t2 = "-k Return\t--help\n\nfin"
last = send_text(41, t2)
want(f"text tid=41 {typed(t2)}")
note("same datagram replayed: seq is old, no TACK; tampered: silent; wrong session: no TACK")
send_text(41, t2, expect_tack=False, pkt=last)
want(f"drop TEXT seq={seq}")
seq += 1
bad = bytearray(text(K, seq, S, 42, "x"))
bad[-1] ^= 1
send_text(42, "x", expect_tack=False, pkt=bytes(bad))
seq += 1
send_text(42, "x", expect_tack=False, pkt=text(K, seq, old, 42, "x"))
want(f"drop TEXT seq={seq}")
note("TEXT and STATE share last_seq")
send_text(42, "ok")
want('text tid=42 "ok"')
send(0, wheel=3)
want("mouse REL_WHEEL=-1")
note("done")
