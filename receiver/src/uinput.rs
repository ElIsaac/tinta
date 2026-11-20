// Two virtual devices through /dev/uinput. Closing the fd destroys the device.
use crate::engine::*;
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;

const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_ABS_SETUP: libc::c_ulong = 0x401c_5504;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
const UI_SET_PROPBIT: libc::c_ulong = 0x4004_556e;
const INPUT_PROP_POINTER: u16 = 0;
const INPUT_PROP_DIRECT: u16 = 1;
const BUS_VIRTUAL: u16 = 6;

const _: () = assert!(std::mem::size_of::<libc::uinput_setup>() == 0x5c);
const _: () = assert!(std::mem::size_of::<libc::uinput_abs_setup>() == 0x1c);

fn ioctl<T>(f: &File, req: libc::c_ulong, arg: T) -> io::Result<()> {
    // SAFETY: req/arg pairs below follow linux/uinput.h
    if unsafe { libc::ioctl(f.as_raw_fd(), req, arg) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open() -> io::Result<File> {
    File::options().write(true).open("/dev/uinput")
}

fn create(f: &File, name: &str, product: u16) -> io::Result<()> {
    let mut s: libc::uinput_setup = unsafe { std::mem::zeroed() };
    s.id.bustype = BUS_VIRTUAL;
    s.id.vendor = 0x5350; // "SP", arbitrary
    s.id.product = product;
    s.id.version = 1;
    for (d, b) in s.name.iter_mut().zip(name.bytes()) {
        *d = b as libc::c_char;
    }
    ioctl(f, UI_DEV_SETUP, &s)?;
    ioctl(f, UI_DEV_CREATE, 0)
}

fn bits(f: &File, req: libc::c_ulong, codes: &[u16]) -> io::Result<()> {
    codes.iter().try_for_each(|&c| ioctl(f, req, c as libc::c_int))
}

pub fn mouse() -> io::Result<File> {
    let f = open()?;
    bits(&f, UI_SET_EVBIT, &[EV_KEY, EV_REL])?;
    bits(&f, UI_SET_KEYBIT, &[BTN_LEFT, BTN_RIGHT, BTN_MIDDLE])?;
    bits(&f, UI_SET_RELBIT, &[REL_X, REL_Y, REL_WHEEL])?;
    create(&f, "spenmouse mouse", 2)?;
    Ok(f)
}

/// Only modifiers are ever pressed, but a full key range makes libinput see a keyboard.
pub fn keyboard() -> io::Result<File> {
    let f = open()?;
    bits(&f, UI_SET_EVBIT, &[EV_KEY])?;
    bits(&f, UI_SET_KEYBIT, &(1..=248).collect::<Vec<u16>>())?;
    create(&f, "spenmouse keyboard", 3)?;
    Ok(f)
}

pub fn hold_key(f: &File, code: u16, down: bool) {
    let mut fr = Frame::default();
    fr.push(EV_KEY, code, down as i32);
    let _ = write(f, &fr);
}

/// res = axis units per mm; libinput ignores tablets with resolution 0.
pub fn tablet(res_x: i32, res_y: i32) -> io::Result<File> {
    let f = open()?;
    bits(&f, UI_SET_EVBIT, &[EV_KEY, EV_ABS])?;
    bits(&f, UI_SET_KEYBIT, &[BTN_TOOL_PEN, BTN_TOUCH, BTN_STYLUS])?;
    // indirect tablet (like a Wacom Intuos): POINTER, never DIRECT
    bits(&f, UI_SET_PROPBIT, &[INPUT_PROP_POINTER])?;
    for (code, resolution) in [(ABS_X, res_x), (ABS_Y, res_y), (ABS_PRESSURE, 0)] {
        ioctl(&f, UI_SET_ABSBIT, code as libc::c_int)?;
        let mut a: libc::uinput_abs_setup = unsafe { std::mem::zeroed() };
        a.code = code;
        a.absinfo.maximum = 65535;
        a.absinfo.resolution = resolution;
        ioctl(&f, UI_ABS_SETUP, &a)?;
    }
    create(&f, "spenmouse tablet", 1)?;
    Ok(f)
}

/// Single-touch touchscreen for mobile mode: the phone's finger lands where it points.
pub fn touch(res_x: i32, res_y: i32) -> io::Result<File> {
    let f = open()?;
    bits(&f, UI_SET_EVBIT, &[EV_KEY, EV_ABS])?;
    bits(&f, UI_SET_KEYBIT, &[BTN_TOUCH])?;
    bits(&f, UI_SET_PROPBIT, &[INPUT_PROP_DIRECT])?;
    for (code, resolution) in [(ABS_X, res_x), (ABS_Y, res_y)] {
        ioctl(&f, UI_SET_ABSBIT, code as libc::c_int)?;
        let mut a: libc::uinput_abs_setup = unsafe { std::mem::zeroed() };
        a.code = code;
        a.absinfo.maximum = 65535;
        a.absinfo.resolution = resolution;
        ioctl(&f, UI_ABS_SETUP, &a)?;
    }
    create(&f, "spenmouse touch", 4)?;
    Ok(f)
}

/// One write(2) per frame: the events plus SYN_REPORT.
pub fn write(mut f: &File, frame: &Frame) -> io::Result<()> {
    let mut buf: [libc::input_event; 7] = unsafe { std::mem::zeroed() };
    for (d, e) in buf.iter_mut().zip(frame.events()) {
        d.type_ = e.kind;
        d.code = e.code;
        d.value = e.value;
    }
    let n = (frame.n + 1) * std::mem::size_of::<libc::input_event>();
    // SAFETY: input_event is plain old data and n <= size_of(buf)
    f.write_all(unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n) })
}
