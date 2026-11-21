#[cfg(target_os = "linux")]
mod imp {
    use crate::engine::Frame;
    use std::fs::File;
    use std::io;

    pub type Mouse = File;
    pub type Keyboard = File;
    pub type Tablet = File;
    pub type Touch = File;

    pub fn mouse() -> io::Result<Mouse> {
        crate::uinput::mouse()
    }

    pub fn keyboard() -> io::Result<Keyboard> {
        crate::uinput::keyboard()
    }

    pub fn tablet(rx: i32, ry: i32) -> io::Result<Tablet> {
        crate::uinput::tablet(rx, ry)
    }

    pub fn touch(rx: i32, ry: i32) -> io::Result<Touch> {
        crate::uinput::touch(rx, ry)
    }

    pub fn write_mouse(m: &Mouse, frame: &Frame) -> io::Result<()> {
        crate::uinput::write(m, frame)
    }

    pub fn write_tablet(t: &Tablet, frame: &Frame) -> io::Result<()> {
        crate::uinput::write(t, frame)
    }

    pub fn write_touch(th: &Touch, frame: &Frame) -> io::Result<()> {
        crate::uinput::write(th, frame)
    }

    pub fn hold_key(k: &Keyboard, code: u16, down: bool) {
        crate::uinput::hold_key(k, code, down);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::engine::*;
    use std::io;

    pub struct Mouse;
    pub struct Keyboard;
    pub struct Tablet;
    pub struct Touch;

    pub fn mouse() -> io::Result<Mouse> {
        Ok(Mouse)
    }

    pub fn keyboard() -> io::Result<Keyboard> {
        Ok(Keyboard)
    }

    pub fn tablet(_rx: i32, _ry: i32) -> io::Result<Tablet> {
        Ok(Tablet)
    }

    pub fn touch(_rx: i32, _ry: i32) -> io::Result<Touch> {
        Ok(Touch)
    }

    pub fn write_mouse(_m: &Mouse, frame: &Frame) -> io::Result<()> {
        let (mut dx, mut dy) = (0i32, 0i32);
        let mut wheel = 0i32;
        for e in frame.events() {
            match (e.kind, e.code) {
                (EV_KEY, BTN_LEFT) => crate::macos::mouse_button(0, e.value != 0),
                (EV_KEY, BTN_RIGHT) => crate::macos::mouse_button(1, e.value != 0),
                (EV_KEY, BTN_MIDDLE) => crate::macos::mouse_button(2, e.value != 0),
                (EV_REL, REL_X) => dx += e.value,
                (EV_REL, REL_Y) => dy += e.value,
                (EV_REL, REL_WHEEL) => wheel += e.value,
                _ => {}
            }
        }
        if dx != 0 || dy != 0 {
            crate::macos::mouse_move_rel(dx, dy);
        }
        if wheel != 0 {
            crate::macos::mouse_wheel(wheel);
        }
        Ok(())
    }

    fn write_abs(is_tablet: bool, frame: &Frame) -> io::Result<()> {
        let (mut has_x, mut abs_x) = (false, 0);
        let (mut has_y, mut abs_y) = (false, 0);
        let (mut has_p, mut pressure) = (false, 0);
        let (mut touch_change, mut touch_down) = (false, false);
        for e in frame.events() {
            match (e.kind, e.code) {
                (EV_ABS, ABS_X) => {
                    has_x = true;
                    abs_x = e.value;
                }
                (EV_ABS, ABS_Y) => {
                    has_y = true;
                    abs_y = e.value;
                }
                (EV_ABS, ABS_PRESSURE) => {
                    has_p = true;
                    pressure = e.value;
                }
                (EV_KEY, BTN_TOUCH) => {
                    touch_change = true;
                    touch_down = e.value != 0;
                }
                _ => {}
            }
        }
        if has_x || has_y || has_p || touch_change {
            crate::macos::abs_event(
                is_tablet,
                has_x,
                abs_x,
                has_y,
                abs_y,
                has_p,
                pressure,
                touch_change,
                touch_down,
            );
        }
        Ok(())
    }

    pub fn write_tablet(_t: &Tablet, frame: &Frame) -> io::Result<()> {
        write_abs(true, frame)
    }

    pub fn write_touch(_th: &Touch, frame: &Frame) -> io::Result<()> {
        write_abs(false, frame)
    }

    pub fn hold_key(_k: &Keyboard, idx: u16, down: bool) {
        crate::macos::hold_mod(idx as usize, down);
    }
}

pub use imp::{
    hold_key, keyboard, mouse, tablet, touch, write_mouse, write_tablet, write_touch, Keyboard,
    Mouse, Tablet, Touch,
};
