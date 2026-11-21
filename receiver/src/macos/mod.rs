#![cfg(target_os = "macos")]

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex};

extern "C" {
    fn tinta_mac_has_accessibility() -> libc::c_int;
    fn tinta_mac_request_accessibility() -> libc::c_int;
    fn tinta_mac_has_screen_recording() -> libc::c_int;
    fn tinta_mac_request_screen_recording() -> libc::c_int;

    fn tinta_mac_set_target_display(display_id: u32);
    fn tinta_mac_set_target_window(wid: u32);
    fn tinta_mac_target_display() -> u32;
    fn tinta_mac_hold_mod(idx: libc::c_int, down: libc::c_int);
    fn tinta_mac_mouse_move_rel(dx: i32, dy: i32);
    fn tinta_mac_mouse_button(btn: libc::c_int, down: libc::c_int);
    fn tinta_mac_mouse_wheel(dy: i32);
    fn tinta_mac_abs_event(
        is_tablet: libc::c_int,
        has_x: libc::c_int,
        abs_x: i32,
        has_y: libc::c_int,
        abs_y: i32,
        has_pressure: libc::c_int,
        pressure: i32,
        touch_change: libc::c_int,
        touch_down: libc::c_int,
    );
    fn tinta_mac_key(keycode: u16, extra_flags: u64);
    fn tinta_mac_type_utf16(units: *const u16, len: usize);

    fn tinta_mac_clip_change_count() -> i64;
    fn tinta_mac_clip_set(utf8: *const libc::c_char, len: usize);
    fn tinta_mac_clip_get(out_len: *mut usize) -> *mut libc::c_char;

    fn tinta_mac_virtual_ensure(phys_w: u32, phys_h: u32, scale: f32) -> u32;
    #[allow(dead_code)]
    fn tinta_mac_virtual_exists() -> libc::c_int;
    fn tinta_mac_virtual_id() -> u32;
    fn tinta_mac_virtual_remove();
    fn tinta_mac_focused_display() -> u32;
    fn tinta_mac_find_display(name: *const libc::c_char) -> u32;
    fn tinta_mac_display_name(display_id: u32) -> *mut libc::c_char;

    fn tinta_mac_capture_start(
        display_id: u32,
        out_w: u32,
        out_h: u32,
        write_fd: libc::c_int,
        err_out: *mut *mut libc::c_char,
    ) -> *mut libc::c_void;
    fn tinta_mac_capture_window_start(
        wid: u32,
        out_w: u32,
        out_h: u32,
        write_fd: libc::c_int,
        err_out: *mut *mut libc::c_char,
    ) -> *mut libc::c_void;
    fn tinta_mac_capture_stop(handle: *mut libc::c_void);

    fn tinta_mac_ws_list(lent_csv: *const libc::c_char) -> *mut libc::c_char;
    fn tinta_mac_frontmost_window(out_pid: *mut i32, out_wid: *mut u32) -> libc::c_int;
    fn tinta_mac_window_exists(wid: u32) -> libc::c_int;
    fn tinta_mac_window_bounds(
        wid: u32,
        x: *mut f64,
        y: *mut f64,
        w: *mut f64,
        h: *mut f64,
    ) -> libc::c_int;
    fn tinta_mac_window_label(wid: u32) -> *mut libc::c_char;
    fn tinta_mac_display_for_window(wid: u32) -> u32;
    fn tinta_mac_move_window(
        pid: i32,
        wid: i32,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        raise_and_activate: libc::c_int,
        old_x: *mut f64,
        old_y: *mut f64,
        old_w: *mut f64,
        old_h: *mut f64,
    ) -> libc::c_int;
    fn tinta_mac_focus_window(pid: i32, wid: i32) -> libc::c_int;
    fn tinta_mac_focused_is_text_input() -> libc::c_int;
    fn tinta_mac_display_bounds(
        display_id: u32,
        x: *mut f64,
        y: *mut f64,
        w: *mut f64,
        h: *mut f64,
    ) -> libc::c_int;
    fn tinta_mac_prevent_sleep(enable: libc::c_int);
    fn tinta_mac_free(ptr: *mut libc::c_void);
}

unsafe fn take_c_string(ptr: *mut libc::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    tinta_mac_free(ptr.cast());
    Some(s)
}

pub fn has_accessibility() -> bool {
    unsafe { tinta_mac_has_accessibility() != 0 }
}

pub fn has_screen_recording() -> bool {
    unsafe { tinta_mac_has_screen_recording() != 0 }
}

pub fn check_permissions() {
    if !has_accessibility() && unsafe { tinta_mac_request_accessibility() == 0 } {
        eprintln!(
            "macos: Accessibility permission not granted (System Settings -> Privacy & Security -> Accessibility); input and window lending may fail"
        );
    }
    if !has_screen_recording() && unsafe { tinta_mac_request_screen_recording() == 0 } {
        eprintln!(
            "macos: Screen Recording permission not granted (System Settings -> Privacy & Security -> Screen Recording); video capture may fail"
        );
    }
}

pub fn set_target_display(display_id: u32) {
    unsafe { tinta_mac_set_target_display(display_id) };
}

pub fn set_target_window(wid: u32) {
    unsafe { tinta_mac_set_target_window(wid) };
}

pub fn target_display() -> u32 {
    unsafe { tinta_mac_target_display() }
}

pub fn hold_mod(idx: usize, down: bool) {
    unsafe { tinta_mac_hold_mod(idx as libc::c_int, down as libc::c_int) };
}

pub fn mouse_move_rel(dx: i32, dy: i32) {
    unsafe { tinta_mac_mouse_move_rel(dx, dy) };
}

pub fn mouse_button(btn: i32, down: bool) {
    unsafe { tinta_mac_mouse_button(btn as libc::c_int, down as libc::c_int) };
}

pub fn mouse_wheel(dy: i32) {
    unsafe { tinta_mac_mouse_wheel(dy) };
}

#[allow(clippy::too_many_arguments)]
pub fn abs_event(
    is_tablet: bool,
    has_x: bool,
    abs_x: i32,
    has_y: bool,
    abs_y: i32,
    has_pressure: bool,
    pressure: i32,
    touch_change: bool,
    touch_down: bool,
) {
    unsafe {
        tinta_mac_abs_event(
            is_tablet as libc::c_int,
            has_x as libc::c_int,
            abs_x,
            has_y as libc::c_int,
            abs_y,
            has_pressure as libc::c_int,
            pressure,
            touch_change as libc::c_int,
            touch_down as libc::c_int,
        );
    }
}

pub fn key(keycode: u16, extra_flags: u64) {
    unsafe { tinta_mac_key(keycode, extra_flags) };
}

pub fn type_utf16(text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    if !units.is_empty() {
        unsafe { tinta_mac_type_utf16(units.as_ptr(), units.len()) };
    }
}

pub fn clip_change_count() -> i64 {
    unsafe { tinta_mac_clip_change_count() }
}

pub fn clip_set(text: &str) {
    let bytes = text.as_bytes();
    let ptr = if bytes.is_empty() {
        std::ptr::null()
    } else {
        bytes.as_ptr().cast::<libc::c_char>()
    };
    unsafe { tinta_mac_clip_set(ptr, bytes.len()) };
}

pub fn clip_get() -> Option<String> {
    let mut len = 0usize;
    let ptr = unsafe { tinta_mac_clip_get(&mut len) };
    if ptr.is_null() {
        return None;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    let text = String::from_utf8_lossy(slice).into_owned();
    unsafe { tinta_mac_free(ptr.cast()) };
    (!text.is_empty()).then_some(text)
}

pub fn virtual_ensure(phys_w: u32, phys_h: u32, scale: f32) -> u32 {
    unsafe { tinta_mac_virtual_ensure(phys_w, phys_h, scale) }
}

#[allow(dead_code)]
pub fn virtual_exists() -> bool {
    unsafe { tinta_mac_virtual_exists() != 0 }
}

pub fn virtual_id() -> u32 {
    unsafe { tinta_mac_virtual_id() }
}

pub fn virtual_remove() {
    unsafe { tinta_mac_virtual_remove() };
}

pub fn focused_display() -> u32 {
    unsafe { tinta_mac_focused_display() }
}

pub fn find_display(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    let id = unsafe { tinta_mac_find_display(c.as_ptr()) };
    (id != 0).then_some(id)
}

pub fn display_name(display_id: u32) -> String {
    unsafe { take_c_string(tinta_mac_display_name(display_id)) }
        .unwrap_or_else(|| format!("display-{display_id}"))
}

pub fn display_bounds(display_id: u32) -> Option<(f64, f64, f64, f64)> {
    let (mut x, mut y, mut w, mut h) = (0.0, 0.0, 0.0, 0.0);
    let ok = unsafe { tinta_mac_display_bounds(display_id, &mut x, &mut y, &mut w, &mut h) };
    (ok != 0).then_some((x, y, w, h))
}

pub fn ws_list(lent_csv: &str) -> String {
    let Ok(c) = CString::new(lent_csv) else {
        return String::new();
    };
    unsafe { take_c_string(tinta_mac_ws_list(c.as_ptr())) }.unwrap_or_default()
}

pub fn move_window(
    pid: i32,
    wid: i32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    raise: bool,
) -> Option<(f64, f64, f64, f64)> {
    let (mut ox, mut oy, mut ow, mut oh) = (0.0, 0.0, 0.0, 0.0);
    let ok = unsafe {
        tinta_mac_move_window(
            pid,
            wid,
            x,
            y,
            w,
            h,
            raise as libc::c_int,
            &mut ox,
            &mut oy,
            &mut ow,
            &mut oh,
        )
    };
    (ok != 0).then_some((ox, oy, ow, oh))
}

pub fn focus_window(pid: i32, wid: i32) -> bool {
    unsafe { tinta_mac_focus_window(pid, wid) != 0 }
}

pub fn frontmost_window() -> Option<(i32, u32)> {
    let (mut pid, mut wid) = (0i32, 0u32);
    let ok = unsafe { tinta_mac_frontmost_window(&mut pid, &mut wid) };
    (ok != 0 && pid > 0 && wid > 0).then_some((pid, wid))
}

pub fn window_exists(wid: u32) -> bool {
    unsafe { tinta_mac_window_exists(wid) != 0 }
}

#[allow(dead_code)]
pub fn window_bounds(wid: u32) -> Option<(f64, f64, f64, f64)> {
    let (mut x, mut y, mut w, mut h) = (0.0, 0.0, 0.0, 0.0);
    let ok = unsafe { tinta_mac_window_bounds(wid, &mut x, &mut y, &mut w, &mut h) };
    (ok != 0).then_some((x, y, w, h))
}

pub fn window_label(wid: u32) -> String {
    unsafe { take_c_string(tinta_mac_window_label(wid)) }
        .unwrap_or_else(|| format!("window-{wid}"))
}

pub fn display_for_window(wid: u32) -> u32 {
    unsafe { tinta_mac_display_for_window(wid) }
}

pub fn focused_is_text_input() -> bool {
    unsafe { tinta_mac_focused_is_text_input() != 0 }
}

pub fn prevent_sleep(enable: bool) {
    unsafe { tinta_mac_prevent_sleep(enable as libc::c_int) };
}

pub struct CaptureHandle {
    ptr: Mutex<*mut libc::c_void>,
}

// SAFETY: TintaCapture synchronizes sample encoding, writes, and teardown internally.
unsafe impl Send for CaptureHandle {}
unsafe impl Sync for CaptureHandle {}

impl CaptureHandle {
    pub fn stop(&self) {
        let raw = {
            let mut g = self.ptr.lock().unwrap();
            std::mem::replace(&mut *g, std::ptr::null_mut())
        };
        if !raw.is_null() {
            unsafe { tinta_mac_capture_stop(raw) };
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct MacCapture {
    handle: Arc<CaptureHandle>,
    reader: File,
}

impl MacCapture {
    fn start_pipe<F>(start_fn: F) -> io::Result<MacCapture>
    where
        F: FnOnce(libc::c_int, *mut *mut libc::c_char) -> *mut libc::c_void,
    {
        let mut fds = [-1i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        // SAFETY: fds[0] is a freshly created pipe read end exclusively owned by `reader`.
        let reader = unsafe { File::from_raw_fd(fds[0]) };
        let mut err_ptr: *mut libc::c_char = std::ptr::null_mut();
        // Ownership of fds[1] transfers to C whether start succeeds or fails.
        let raw = start_fn(fds[1], &mut err_ptr);
        if raw.is_null() {
            let msg = unsafe { take_c_string(err_ptr) }
                .unwrap_or_else(|| "ScreenCaptureKit failed to start".to_string());
            return Err(io::Error::other(msg));
        }
        Ok(MacCapture {
            handle: Arc::new(CaptureHandle { ptr: Mutex::new(raw) }),
            reader,
        })
    }

    pub fn start(display_id: u32, out_w: u32, out_h: u32) -> io::Result<MacCapture> {
        Self::start_pipe(|write_fd, err_ptr| unsafe {
            tinta_mac_capture_start(display_id, out_w, out_h, write_fd, err_ptr)
        })
    }

    pub fn start_window(wid: u32, out_w: u32, out_h: u32) -> io::Result<MacCapture> {
        Self::start_pipe(|write_fd, err_ptr| unsafe {
            tinta_mac_capture_window_start(wid, out_w, out_h, write_fd, err_ptr)
        })
    }

    pub fn handle(&self) -> Arc<CaptureHandle> {
        self.handle.clone()
    }

    #[allow(dead_code)]
    pub fn stop(&self) {
        self.handle.stop();
    }
}

impl Read for MacCapture {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        self.handle.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interactive_display_and_clipboard_queries() {
        let _ = has_accessibility();
        let _ = has_screen_recording();
        let _ = clip_change_count();
        let _ = focused_is_text_input();
        assert!(!virtual_exists());
        assert_eq!(virtual_id(), 0);

        let main_id = find_display("main").expect("main display should exist");
        assert_ne!(main_id, 0);
        assert_eq!(find_display(""), None);
        assert_eq!(find_display("0"), None);
        assert_eq!(find_display(&main_id.to_string()), Some(main_id));

        let name = display_name(main_id);
        assert!(!name.is_empty());
        if !name.starts_with("display-") {
            assert!(find_display(&name).is_some());
        }

        let bounds = display_bounds(main_id).expect("main display should have bounds");
        assert!(bounds.2 > 0.0 && bounds.3 > 0.0);
        assert_eq!(display_bounds(0), None);

        let focused = focused_display();
        assert_ne!(focused, 0);

        let list = ws_list("");
        assert!(list.lines().any(|l| l.starts_with(&format!("{main_id}\t"))));

        let _ = frontmost_window();
        assert!(!window_exists(0));
        assert_eq!(window_bounds(0), None);
        let _ = display_for_window(0);
        assert!(!window_label(0).is_empty());

        prevent_sleep(true);
        prevent_sleep(false);
        prevent_sleep(false);
    }
}
