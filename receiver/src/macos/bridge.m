#import <Foundation/Foundation.h>
#import <CoreFoundation/CoreFoundation.h>
#import <CoreGraphics/CoreGraphics.h>
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <Carbon/Carbon.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <VideoToolbox/VideoToolbox.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <IOKit/pwr_mgt/IOPMLib.h>

#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <math.h>
#include <poll.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* -------------------------------------------------------------------------- */
/* Permissions                                                                */
/* -------------------------------------------------------------------------- */

int tinta_mac_has_accessibility(void) {
    return AXIsProcessTrusted() ? 1 : 0;
}

int tinta_mac_request_accessibility(void) {
    @autoreleasepool {
        NSDictionary *opts = @{(__bridge id)kAXTrustedCheckOptionPrompt: @YES};
        return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)opts) ? 1 : 0;
    }
}

int tinta_mac_has_screen_recording(void) {
    return CGPreflightScreenCaptureAccess() ? 1 : 0;
}

int tinta_mac_request_screen_recording(void) {
    return CGRequestScreenCaptureAccess() ? 1 : 0;
}

/* -------------------------------------------------------------------------- */
/* Input & Modifiers (CGEvent)                                                */
/* -------------------------------------------------------------------------- */

static pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
static CGEventSourceRef g_src = NULL;
static CGEventFlags g_held_flags = 0;
static int g_mouse_left = 0;
static int g_mouse_right = 0;
static int g_mouse_middle = 0;
static int g_pen_touch = 0;
static int g_finger_touch = 0;
static uint32_t g_target_display = 0;
static uint32_t g_target_window = 0;
static int32_t g_abs_x = 0;
static int32_t g_abs_y = 0;
static int32_t g_pressure = 0;
static CFAbsoluteTime g_last_click_time = 0.0;
static CGPoint g_last_click_pos = {0.0, 0.0};
static int g_last_click_btn = -1;
static int64_t g_click_count = 0;
static CGPoint g_cursor_pos = {0.0, 0.0};
static int g_cursor_valid = 0;
static CFAbsoluteTime g_last_move_time = 0.0;

static BOOL lookup_cgwindow_info(int32_t wid, CGRect *out_bounds, NSString **out_title);
static uint32_t display_for_rect(CGRect r);
uint32_t tinta_mac_display_for_window(uint32_t wid);

static CGEventSourceRef event_source(void) {
    if (!g_src) {
        g_src = CGEventSourceCreate(kCGEventSourceStateHIDSystemState);
        if (g_src) {
            CGEventSourceSetLocalEventsSuppressionInterval(g_src, 0.0);
        }
    }
    return g_src;
}

static inline int effective_left(void) {
    return g_mouse_left || g_pen_touch || g_finger_touch;
}

static CGPoint current_cursor_pos(void) {
    CGEventRef tmp = CGEventCreate(NULL);
    CGPoint p = CGPointZero;
    if (tmp) {
        p = CGEventGetLocation(tmp);
        CFRelease(tmp);
    }
    return p;
}

static CGPoint clamp_to_active_displays(CGPoint pt) {
    CGDirectDisplayID ids[32];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(32, ids, &count) != kCGErrorSuccess || count == 0) {
        CGRect main_b = CGDisplayBounds(CGMainDisplayID());
        if (CGRectIsEmpty(main_b)) {
            return pt;
        }
        double max_x = main_b.origin.x + fmax(main_b.size.width - 1.0, 0.0);
        double max_y = main_b.origin.y + fmax(main_b.size.height - 1.0, 0.0);
        return CGPointMake(
            fmin(fmax(pt.x, main_b.origin.x), max_x),
            fmin(fmax(pt.y, main_b.origin.y), max_y)
        );
    }

    for (uint32_t i = 0; i < count; i++) {
        CGRect b = CGDisplayBounds(ids[i]);
        if (CGRectContainsPoint(b, pt)) {
            return pt;
        }
    }

    CGPoint best = pt;
    double best_d2 = INFINITY;
    for (uint32_t i = 0; i < count; i++) {
        CGRect b = CGDisplayBounds(ids[i]);
        if (CGRectIsEmpty(b)) {
            continue;
        }
        double max_x = b.origin.x + fmax(b.size.width - 1.0, 0.0);
        double max_y = b.origin.y + fmax(b.size.height - 1.0, 0.0);
        double cx = fmin(fmax(pt.x, b.origin.x), max_x);
        double cy = fmin(fmax(pt.y, b.origin.y), max_y);
        double dx = pt.x - cx;
        double dy = pt.y - cy;
        double d2 = dx * dx + dy * dy;
        if (d2 < best_d2) {
            best_d2 = d2;
            best = CGPointMake(cx, cy);
        }
    }
    return best;
}

static int64_t next_click_state(int btn, CGPoint pos, int down) {
    if (!down) {
        return g_click_count > 0 ? g_click_count : 1;
    }
    CFAbsoluteTime now = CFAbsoluteTimeGetCurrent();
    double dx = pos.x - g_last_click_pos.x;
    double dy = pos.y - g_last_click_pos.y;
    if (g_last_click_btn == btn && (now - g_last_click_time) <= 0.5 && (dx * dx + dy * dy) <= 36.0) {
        g_click_count = (g_click_count >= 3) ? 1 : (g_click_count + 1);
    } else {
        g_click_count = 1;
    }
    g_last_click_time = now;
    g_last_click_pos = pos;
    g_last_click_btn = btn;
    return g_click_count;
}

static void apply_tablet_fields(CGEventRef ev, int is_tablet, int32_t pressure) {
    if (is_tablet && pressure > 0) {
        double p = fmin(fmax((double)pressure / 65535.0, 0.0), 1.0);
        CGEventSetIntegerValueField(ev, kCGMouseEventSubtype, kCGEventMouseSubtypeTabletPoint);
        CGEventSetDoubleValueField(ev, kCGTabletEventPointPressure, p);
        CGEventSetDoubleValueField(ev, kCGMouseEventPressure, p);
    }
}

static void post_move_at(CGEventSourceRef src, CGPoint dst, int32_t dx, int32_t dy, int is_tablet, int32_t pressure) {
    CGEventType type = kCGEventMouseMoved;
    CGMouseButton btn = kCGMouseButtonLeft;
    if (effective_left()) {
        type = kCGEventLeftMouseDragged;
        btn = kCGMouseButtonLeft;
    } else if (g_mouse_right) {
        type = kCGEventRightMouseDragged;
        btn = kCGMouseButtonRight;
    } else if (g_mouse_middle) {
        type = kCGEventOtherMouseDragged;
        btn = kCGMouseButtonCenter;
    }
    CGEventRef ev = CGEventCreateMouseEvent(src, type, dst, btn);
    if (!ev) {
        return;
    }
    if (dx != 0 || dy != 0) {
        CGEventSetIntegerValueField(ev, kCGMouseEventDeltaX, dx);
        CGEventSetIntegerValueField(ev, kCGMouseEventDeltaY, dy);
    }
    if (type == kCGEventOtherMouseDragged) {
        CGEventSetIntegerValueField(ev, kCGMouseEventButtonNumber, 2);
    }
    apply_tablet_fields(ev, is_tablet, pressure);
    CGEventSetFlags(ev, g_held_flags);
    CGEventPost(kCGHIDEventTap, ev);
    CFRelease(ev);
}

static void post_button_at(CGEventSourceRef src, int btn, int down, CGPoint pos, int is_tablet, int32_t pressure) {
    CGEventType type;
    CGMouseButton cbtn;
    if (btn == 0) {
        type = down ? kCGEventLeftMouseDown : kCGEventLeftMouseUp;
        cbtn = kCGMouseButtonLeft;
    } else if (btn == 1) {
        type = down ? kCGEventRightMouseDown : kCGEventRightMouseUp;
        cbtn = kCGMouseButtonRight;
    } else {
        type = down ? kCGEventOtherMouseDown : kCGEventOtherMouseUp;
        cbtn = kCGMouseButtonCenter;
    }
    int64_t clicks = next_click_state(btn, pos, down);
    CGEventRef ev = CGEventCreateMouseEvent(src, type, pos, cbtn);
    if (!ev) {
        return;
    }
    CGEventSetIntegerValueField(ev, kCGMouseEventClickState, clicks);
    if (btn == 2) {
        CGEventSetIntegerValueField(ev, kCGMouseEventButtonNumber, 2);
    }
    if (down) {
        apply_tablet_fields(ev, is_tablet, pressure);
    }
    CGEventSetFlags(ev, g_held_flags);
    CGEventPost(kCGHIDEventTap, ev);
    CFRelease(ev);
}

void tinta_mac_set_target_display(uint32_t display_id) {
    pthread_mutex_lock(&g_lock);
    g_target_display = display_id;
    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_set_target_window(uint32_t wid) {
    pthread_mutex_lock(&g_lock);
    g_target_window = wid;
    pthread_mutex_unlock(&g_lock);
}

uint32_t tinta_mac_target_display(void) {
    pthread_mutex_lock(&g_lock);
    uint32_t did = g_target_display ? g_target_display : CGMainDisplayID();
    pthread_mutex_unlock(&g_lock);
    return did;
}

void tinta_mac_hold_mod(int idx, int down) {
    CGKeyCode kc;
    CGEventFlags mask;
    switch (idx) {
        case 0:
            kc = kVK_Command;
            mask = kCGEventFlagMaskCommand;
            break;
        case 1:
            kc = kVK_Control;
            mask = kCGEventFlagMaskControl;
            break;
        case 2:
            kc = kVK_Option;
            mask = kCGEventFlagMaskAlternate;
            break;
        case 3:
            kc = kVK_Shift;
            mask = kCGEventFlagMaskShift;
            break;
        default:
            return;
    }

    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();
    if (down) {
        g_held_flags |= mask;
    } else {
        g_held_flags &= ~mask;
    }
    CGEventRef ev = CGEventCreateKeyboardEvent(src, kc, down != 0);
    if (ev) {
        CGEventSetType(ev, kCGEventFlagsChanged);
        CGEventSetFlags(ev, g_held_flags);
        CGEventPost(kCGHIDEventTap, ev);
        CFRelease(ev);
    }
    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_mouse_move_rel(int32_t dx, int32_t dy) {
    if (dx == 0 && dy == 0) {
        return;
    }
    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();
    CFAbsoluteTime now = CFAbsoluteTimeGetCurrent();
    CGPoint cur = (g_cursor_valid && (now - g_last_move_time) <= 0.05)
        ? g_cursor_pos
        : current_cursor_pos();
    CGPoint dst = clamp_to_active_displays(CGPointMake(cur.x + (double)dx, cur.y + (double)dy));
    post_move_at(src, dst, dx, dy, 0, 0);
    g_cursor_pos = dst;
    g_cursor_valid = 1;
    g_last_move_time = now;
    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_mouse_button(int btn, int down) {
    int d = down ? 1 : 0;
    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();
    if (btn == 0) {
        int prev = effective_left();
        g_mouse_left = d;
        int next = effective_left();
        if (prev == next) {
            pthread_mutex_unlock(&g_lock);
            return;
        }
    } else if (btn == 1) {
        if (g_mouse_right == d) {
            pthread_mutex_unlock(&g_lock);
            return;
        }
        g_mouse_right = d;
    } else if (btn == 2) {
        if (g_mouse_middle == d) {
            pthread_mutex_unlock(&g_lock);
            return;
        }
        g_mouse_middle = d;
    } else {
        pthread_mutex_unlock(&g_lock);
        return;
    }
    CFAbsoluteTime now = CFAbsoluteTimeGetCurrent();
    CGPoint pos = (g_cursor_valid && (now - g_last_move_time) <= 0.15)
        ? g_cursor_pos
        : current_cursor_pos();
    post_button_at(src, btn, d, pos, 0, 0);
    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_mouse_wheel(int32_t dy) {
    if (dy == 0) {
        return;
    }
    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();
    CGEventRef ev = CGEventCreateScrollWheelEvent(src, kCGScrollEventUnitLine, 1, dy * 3);
    if (ev) {
        CGEventSetFlags(ev, g_held_flags);
        CGEventPost(kCGHIDEventTap, ev);
        CFRelease(ev);
    }
    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_abs_event(
    int is_tablet,
    int has_x,
    int32_t abs_x,
    int has_y,
    int32_t abs_y,
    int has_pressure,
    int32_t pressure,
    int touch_change,
    int touch_down
) {
    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();

    if (has_x) {
        if (abs_x < 0) abs_x = 0;
        if (abs_x > 65535) abs_x = 65535;
        g_abs_x = abs_x;
    }
    if (has_y) {
        if (abs_y < 0) abs_y = 0;
        if (abs_y > 65535) abs_y = 65535;
        g_abs_y = abs_y;
    }
    if (has_pressure) {
        if (pressure < 0) pressure = 0;
        if (pressure > 65535) pressure = 65535;
        g_pressure = pressure;
    }

    uint32_t wid = g_target_window;
    uint32_t did = g_target_display ? g_target_display : CGMainDisplayID();
    CGRect bounds = CGRectZero;
    BOOL have_win_bounds = NO;
    if (wid != 0) {
        @autoreleasepool {
            CGRect wb = CGRectZero;
            if (lookup_cgwindow_info((int32_t)wid, &wb, NULL) && !CGRectIsEmpty(wb)) {
                bounds = wb;
                have_win_bounds = YES;
                did = display_for_rect(bounds);
            }
        }
    }
    if (!have_win_bounds) {
        bounds = CGDisplayBounds(did);
        if (CGRectIsEmpty(bounds)) {
            did = CGMainDisplayID();
            bounds = CGDisplayBounds(did);
        }
    }
    double px = bounds.origin.x + ((double)g_abs_x / 65535.0) * fmax(bounds.size.width - 1.0, 0.0);
    double py = bounds.origin.y + ((double)g_abs_y / 65535.0) * fmax(bounds.size.height - 1.0, 0.0);
    CGPoint pt = CGPointMake(px, py);

    int32_t eff_p = is_tablet ? (has_pressure ? pressure : g_pressure) : 0;
    int prev_left = effective_left();
    if (touch_change) {
        int td = touch_down ? 1 : 0;
        if (is_tablet) {
            g_pen_touch = td;
        } else {
            g_finger_touch = td;
        }
    }
    int next_left = effective_left();

    int should_post = (prev_left != next_left) || has_x || has_y || (is_tablet && has_pressure && eff_p > 0);
    if (should_post) {
        CFAbsoluteTime now = CFAbsoluteTimeGetCurrent();
        CGPoint prev_pos = (g_cursor_valid && (now - g_last_move_time) <= 0.15)
            ? g_cursor_pos
            : current_cursor_pos();
        CGDirectDisplayID prev_did = 0;
        uint32_t dcount = 0;
        if (CGGetDisplaysWithPoint(prev_pos, 1, &prev_did, &dcount) != kCGErrorSuccess || dcount == 0 || prev_did != did) {
            // Warp explicitly so macOS crosses onto CGVirtualDisplay before the event posts.
            CGWarpMouseCursorPosition(pt);
            CGAssociateMouseAndMouseCursorPosition(true);
        }

        if (prev_left != next_left) {
            if (has_x || has_y) {
                if (next_left) {
                    // Move before MouseDown so hover targets under the touch point update first.
                    CGEventRef mv = CGEventCreateMouseEvent(src, kCGEventMouseMoved, pt, kCGMouseButtonLeft);
                    if (mv) {
                        apply_tablet_fields(mv, is_tablet, eff_p);
                        CGEventSetFlags(mv, g_held_flags);
                        CGEventPost(kCGHIDEventTap, mv);
                        CFRelease(mv);
                    }
                } else {
                    CGEventRef dr = CGEventCreateMouseEvent(src, kCGEventLeftMouseDragged, pt, kCGMouseButtonLeft);
                    if (dr) {
                        apply_tablet_fields(dr, is_tablet, eff_p);
                        CGEventSetFlags(dr, g_held_flags);
                        CGEventPost(kCGHIDEventTap, dr);
                        CFRelease(dr);
                    }
                }
            }
            post_button_at(src, 0, next_left, pt, is_tablet, eff_p);
            if (!next_left && is_tablet) {
                g_pressure = 0;
            }
        } else {
            post_move_at(src, pt, 0, 0, is_tablet, eff_p);
        }

        g_cursor_pos = pt;
        g_cursor_valid = 1;
        g_last_move_time = now;
    }

    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_key(uint16_t keycode, uint64_t extra_flags) {
    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();
    CGEventFlags flags = g_held_flags | (CGEventFlags)extra_flags;
    CGEventRef down = CGEventCreateKeyboardEvent(src, (CGKeyCode)keycode, true);
    if (down) {
        CGEventSetFlags(down, flags);
        CGEventPost(kCGHIDEventTap, down);
        CFRelease(down);
    }
    CGEventRef up = CGEventCreateKeyboardEvent(src, (CGKeyCode)keycode, false);
    if (up) {
        CGEventSetFlags(up, flags);
        CGEventPost(kCGHIDEventTap, up);
        CFRelease(up);
    }
    pthread_mutex_unlock(&g_lock);
}

void tinta_mac_type_utf16(const uint16_t *units, size_t len) {
    if (!units || len == 0) {
        return;
    }
    pthread_mutex_lock(&g_lock);
    CGEventSourceRef src = event_source();
    CGEventFlags flags = g_held_flags;
    size_t off = 0;
    while (off < len) {
        size_t n = len - off;
        if (n > 20) {
            n = 20;
            // Avoid splitting a UTF-16 surrogate pair across events.
            uint16_t last = units[off + n - 1];
            if (last >= 0xD800 && last <= 0xDBFF) {
                n--;
            }
        }
        CGEventRef down = CGEventCreateKeyboardEvent(src, 0, true);
        if (down) {
            CGEventKeyboardSetUnicodeString(down, (UniCharCount)n, (const UniChar *)(units + off));
            CGEventSetFlags(down, flags);
            CGEventPost(kCGHIDEventTap, down);
            CFRelease(down);
        }
        CGEventRef up = CGEventCreateKeyboardEvent(src, 0, false);
        if (up) {
            CGEventKeyboardSetUnicodeString(up, (UniCharCount)n, (const UniChar *)(units + off));
            CGEventSetFlags(up, flags);
            CGEventPost(kCGHIDEventTap, up);
            CFRelease(up);
        }
        off += n;
    }
    pthread_mutex_unlock(&g_lock);
}

/* -------------------------------------------------------------------------- */
/* Clipboard (NSPasteboard)                                                   */
/* -------------------------------------------------------------------------- */

int64_t tinta_mac_clip_change_count(void) {
    @autoreleasepool {
        return (int64_t)[[NSPasteboard generalPasteboard] changeCount];
    }
}

void tinta_mac_clip_set(const char *utf8, size_t len) {
    @autoreleasepool {
        if (!utf8 && len > 0) {
            return;
        }
        NSString *str = (len == 0)
            ? @""
            : [[NSString alloc] initWithBytes:utf8 length:len encoding:NSUTF8StringEncoding];
        if (!str) {
            return;
        }
        NSPasteboard *pb = [NSPasteboard generalPasteboard];
        [pb clearContents];
        [pb setString:str forType:NSPasteboardTypeString];
    }
}

char *tinta_mac_clip_get(size_t *out_len) {
    @autoreleasepool {
        if (out_len) {
            *out_len = 0;
        }
        NSString *str = [[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeString];
        if (!str || str.length == 0) {
            return NULL;
        }
        NSData *data = [str dataUsingEncoding:NSUTF8StringEncoding];
        if (!data || data.length == 0) {
            return NULL;
        }
        char *buf = (char *)malloc(data.length + 1);
        if (!buf) {
            return NULL;
        }
        memcpy(buf, data.bytes, data.length);
        buf[data.length] = '\0';
        if (out_len) {
            *out_len = data.length;
        }
        return buf;
    }
}

/* -------------------------------------------------------------------------- */
/* Virtual Display (CGVirtualDisplay via NSClassFromString) & Display Helpers */
/* -------------------------------------------------------------------------- */

@interface CGVirtualDisplayDescriptor : NSObject
@property (nonatomic, assign) uint32_t vendorID;
@property (nonatomic, assign) uint32_t productID;
@property (nonatomic, assign) uint32_t serialNum;
@property (nonatomic, assign) uint32_t serialNumber;
@property (nonatomic, copy) NSString *name;
@property (nonatomic, strong) dispatch_queue_t queue;
@property (nonatomic, strong) dispatch_queue_t dispatchQueue;
@property (nonatomic, assign) uint32_t maxPixelsWide;
@property (nonatomic, assign) uint32_t maxPixelsHigh;
@property (nonatomic, assign) CGSize sizeInMillimeters;
@property (nonatomic, assign) CGPoint redPrimary;
@property (nonatomic, assign) CGPoint greenPrimary;
@property (nonatomic, assign) CGPoint bluePrimary;
@property (nonatomic, assign) CGPoint whitePoint;
@property (nonatomic, copy) void (^terminationHandler)(id, id);
@end

@interface CGVirtualDisplayMode : NSObject
- (instancetype)initWithWidth:(uint32_t)width height:(uint32_t)height refreshRate:(double)refreshRate;
@end

@interface CGVirtualDisplaySettings : NSObject
@property (nonatomic, assign) uint32_t hiDPI;
@property (nonatomic, copy) NSArray *modes;
@end

@interface CGVirtualDisplay : NSObject
@property (nonatomic, readonly) uint32_t displayID;
- (instancetype)initWithDescriptor:(CGVirtualDisplayDescriptor *)descriptor;
- (BOOL)applySettings:(CGVirtualDisplaySettings *)settings;
@end

static pthread_mutex_t g_vlock = PTHREAD_MUTEX_INITIALIZER;
static CGVirtualDisplay *g_vdisplay = nil;
static uint32_t g_vdisplay_id = 0;
static uint32_t g_vmax_w = 0;
static uint32_t g_vmax_h = 0;
static uint32_t g_vhidpi = 0;

uint32_t tinta_mac_virtual_ensure(uint32_t phys_w, uint32_t phys_h, float scale) {
    @autoreleasepool {
        Class descCls = NSClassFromString(@"CGVirtualDisplayDescriptor");
        Class modeCls = NSClassFromString(@"CGVirtualDisplayMode");
        Class setCls = NSClassFromString(@"CGVirtualDisplaySettings");
        Class dispCls = NSClassFromString(@"CGVirtualDisplay");
        if (!descCls || !modeCls || !setCls || !dispCls) {
            return 0;
        }

        float s = (scale > 0.1f && isfinite(scale)) ? scale : 1.5f;
        uint32_t mode_w = (uint32_t)llroundf((float)phys_w / s) & ~1U;
        uint32_t mode_h = (uint32_t)llroundf((float)phys_h / s) & ~1U;
        if (mode_w < 320) mode_w = 320;
        if (mode_h < 320) mode_h = 320;
        if (mode_w > 6144) mode_w = 6144;
        if (mode_h > 4096) mode_h = 4096;

        uint32_t hidpi = (s >= 1.5f) ? 1 : 0;
        uint32_t max_w = hidpi ? (mode_w * 2 > 6144 ? 6144 : mode_w * 2) : mode_w;
        uint32_t max_h = hidpi ? (mode_h * 2 > 4096 ? 4096 : mode_h * 2) : mode_h;

        CGVirtualDisplayMode *mode = [((CGVirtualDisplayMode *)[modeCls alloc])
            initWithWidth:mode_w height:mode_h refreshRate:60.0];
        if (!mode) {
            return 0;
        }
        CGVirtualDisplaySettings *settings = [[setCls alloc] init];
        settings.hiDPI = hidpi;
        settings.modes = @[mode];

        CFTypeRef stale = NULL;
        uint32_t out_id = 0;

        pthread_mutex_lock(&g_vlock);
        if (g_vdisplay && max_w <= g_vmax_w && max_h <= g_vmax_h && g_vhidpi == hidpi) {
            if ([g_vdisplay applySettings:settings]) {
                out_id = g_vdisplay_id;
                pthread_mutex_unlock(&g_vlock);
                return out_id;
            }
        }

        if (g_vdisplay) {
            stale = CFBridgingRetain(g_vdisplay);
            g_vdisplay = nil;
        }
        g_vdisplay_id = 0;
        g_vmax_w = 0;
        g_vmax_h = 0;
        g_vhidpi = 0;

        CGVirtualDisplayDescriptor *desc = [[descCls alloc] init];
        desc.vendorID = 0x5350;
        desc.productID = 0x0001;
        desc.serialNum = 1;
        desc.serialNumber = 1;
        desc.name = @"spenmouse";
        dispatch_queue_t q = dispatch_get_global_queue(QOS_CLASS_USER_INTERACTIVE, 0);
        desc.queue = q;
        desc.dispatchQueue = q;
        desc.maxPixelsWide = max_w;
        desc.maxPixelsHigh = max_h;
        desc.sizeInMillimeters = CGSizeMake((double)mode_w * 25.4 / 140.0, (double)mode_h * 25.4 / 140.0);
        desc.redPrimary = CGPointMake(0.640, 0.330);
        desc.greenPrimary = CGPointMake(0.300, 0.600);
        desc.bluePrimary = CGPointMake(0.150, 0.060);
        desc.whitePoint = CGPointMake(0.3127, 0.3290);

        CGVirtualDisplay *disp = [((CGVirtualDisplay *)[dispCls alloc]) initWithDescriptor:desc];
        if (disp && [disp applySettings:settings] && disp.displayID != 0) {
            g_vdisplay = disp;
            g_vdisplay_id = disp.displayID;
            g_vmax_w = max_w;
            g_vmax_h = max_h;
            g_vhidpi = hidpi;
            out_id = g_vdisplay_id;
        }
        pthread_mutex_unlock(&g_vlock);

        if (stale) {
            CFRelease(stale);
        }
        return out_id;
    }
}

int tinta_mac_virtual_exists(void) {
    pthread_mutex_lock(&g_vlock);
    int exists = (g_vdisplay != nil && g_vdisplay_id != 0) ? 1 : 0;
    pthread_mutex_unlock(&g_vlock);
    return exists;
}

uint32_t tinta_mac_virtual_id(void) {
    pthread_mutex_lock(&g_vlock);
    uint32_t did = (g_vdisplay != nil) ? g_vdisplay_id : 0;
    pthread_mutex_unlock(&g_vlock);
    return did;
}

void tinta_mac_virtual_remove(void) {
    @autoreleasepool {
        pthread_mutex_lock(&g_vlock);
        CFTypeRef old = g_vdisplay ? CFBridgingRetain(g_vdisplay) : NULL;
        g_vdisplay = nil;
        g_vdisplay_id = 0;
        g_vmax_w = 0;
        g_vmax_h = 0;
        g_vhidpi = 0;
        pthread_mutex_unlock(&g_vlock);
        if (old) {
            CFRelease(old);
        }
    }
}

uint32_t tinta_mac_focused_display(void) {
    CGPoint loc = current_cursor_pos();
    CGDirectDisplayID did = 0;
    uint32_t count = 0;
    if (CGGetDisplaysWithPoint(loc, 1, &did, &count) == kCGErrorSuccess && count > 0 && did != 0) {
        return did;
    }
    return CGMainDisplayID();
}

static NSString *display_name_ns(uint32_t display_id) {
    uint32_t virt_id = tinta_mac_virtual_id();
    if (virt_id != 0 && display_id == virt_id) {
        return @"spenmouse";
    }
    for (NSScreen *s in [NSScreen screens]) {
        NSNumber *num = s.deviceDescription[@"NSScreenNumber"];
        if (num && [num unsignedIntValue] == display_id) {
            NSString *ln = s.localizedName;
            if (ln && ln.length > 0) {
                return ln;
            }
        }
    }
    return CGDisplayIsBuiltin(display_id)
        ? @"Built-in"
        : [NSString stringWithFormat:@"Display %u", display_id];
}

uint32_t tinta_mac_find_display(const char *name) {
    @autoreleasepool {
        if (!name || !*name) {
            return 0;
        }
        if (strcasecmp(name, "main") == 0) {
            return CGMainDisplayID();
        }
        if (strcasecmp(name, "spenmouse") == 0) {
            return tinta_mac_virtual_id();
        }

        CGDirectDisplayID ids[32];
        uint32_t count = 0;
        CGGetActiveDisplayList(32, ids, &count);

        char *end = NULL;
        unsigned long parsed = strtoul(name, &end, 10);
        if (end && *end == '\0' && parsed > 0 && parsed <= UINT32_MAX) {
            uint32_t want = (uint32_t)parsed;
            for (uint32_t i = 0; i < count; i++) {
                if (ids[i] == want) {
                    return want;
                }
            }
        }

        NSString *target = [NSString stringWithUTF8String:name];
        if (!target) {
            return 0;
        }
        for (NSScreen *s in [NSScreen screens]) {
            if (s.localizedName && [s.localizedName caseInsensitiveCompare:target] == NSOrderedSame) {
                NSNumber *num = s.deviceDescription[@"NSScreenNumber"];
                if (num) {
                    return [num unsignedIntValue];
                }
            }
        }
        return 0;
    }
}

char *tinta_mac_display_name(uint32_t display_id) {
    @autoreleasepool {
        NSString *name = display_name_ns(display_id);
        return strdup([name UTF8String]);
    }
}

int tinta_mac_display_bounds(uint32_t display_id, double *x, double *y, double *w, double *h) {
    CGDirectDisplayID ids[32];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(32, ids, &count) != kCGErrorSuccess) {
        return 0;
    }
    int found = 0;
    for (uint32_t i = 0; i < count; i++) {
        if (ids[i] == display_id) {
            found = 1;
            break;
        }
    }
    if (!found) {
        return 0;
    }
    CGRect b = CGDisplayBounds(display_id);
    if (CGRectIsEmpty(b)) {
        return 0;
    }
    if (x) *x = b.origin.x;
    if (y) *y = b.origin.y;
    if (w) *w = b.size.width;
    if (h) *h = b.size.height;
    return 1;
}

/* -------------------------------------------------------------------------- */
/* ScreenCaptureKit + VideoToolbox H.264 Annex-B                              */
/* -------------------------------------------------------------------------- */

static void set_err(char **err_out, const char *msg) {
    if (err_out && !*err_out && msg) {
        *err_out = strdup(msg);
    }
}

static void configure_pipe_write_fd(int fd) {
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags >= 0) {
        fcntl(fd, F_SETFL, flags | O_NONBLOCK);
    }
#ifdef F_SETNOSIGPIPE
    fcntl(fd, F_SETNOSIGPIPE, 1);
#endif
}

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wunguarded-availability-new"

@interface TintaCapture : NSObject <SCStreamOutput, SCStreamDelegate> {
@public
    pthread_mutex_t _encode_lock;
    pthread_mutex_t _write_lock;
    atomic_bool _stopped;
    int _write_fd;
    int _nal_len_size;
    VTCompressionSessionRef _vt;
    SCStream *_stream;
    dispatch_queue_t _sample_queue;
    NSMutableData *_annexb;
    uint64_t _frame_idx;
    CMTime _last_pts;
    BOOL _force_keyframe;
}
@end

static int write_all_fd(TintaCapture *cap, const uint8_t *buf, size_t len) {
    size_t off = 0;
    while (off < len) {
        if (atomic_load(&cap->_stopped) || cap->_write_fd < 0) {
            return -1;
        }
        ssize_t n = write(cap->_write_fd, buf + off, len - off);
        if (n > 0) {
            off += (size_t)n;
            continue;
        }
        if (n < 0 && errno == EINTR) {
            continue;
        }
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            struct pollfd pfd = { .fd = cap->_write_fd, .events = POLLOUT, .revents = 0 };
            int r = poll(&pfd, 1, 50);
            if (r < 0 && errno != EINTR) {
                return -1;
            }
            if (pfd.revents & (POLLERR | POLLHUP | POLLNVAL)) {
                return -1;
            }
            continue;
        }
        return -1;
    }
    return 0;
}

static void vt_output_callback(
    void *outputCallbackRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTEncodeInfoFlags infoFlags,
    CMSampleBufferRef sampleBuffer
) {
    if (status != noErr || !sampleBuffer || (infoFlags & kVTEncodeInfo_FrameDropped) || !outputCallbackRefCon) {
        return;
    }
    TintaCapture *cap = (__bridge TintaCapture *)outputCallbackRefCon;
    if (atomic_load(&cap->_stopped)) {
        return;
    }

    @autoreleasepool {
        BOOL is_sync = YES;
        CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, false);
        if (attachments && CFArrayGetCount(attachments) > 0) {
            CFDictionaryRef dict = (CFDictionaryRef)CFArrayGetValueAtIndex(attachments, 0);
            const void *not_sync = NULL;
            if (CFDictionaryGetValueIfPresent(dict, kCMSampleAttachmentKey_NotSync, &not_sync) && not_sync == kCFBooleanTrue) {
                is_sync = NO;
            }
        }

        static const uint8_t start_code[4] = {0x00, 0x00, 0x00, 0x01};

        pthread_mutex_lock(&cap->_write_lock);
        if (atomic_load(&cap->_stopped) || cap->_write_fd < 0) {
            pthread_mutex_unlock(&cap->_write_lock);
            return;
        }

        [cap->_annexb setLength:0];
        int nal_hdr_len = cap->_nal_len_size > 0 ? cap->_nal_len_size : 4;

        CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sampleBuffer);
        if (fmt) {
            size_t ps_count = 0;
            int hdr_len_out = 0;
            if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, 0, NULL, NULL, &ps_count, &hdr_len_out) == noErr) {
                if (hdr_len_out == 1 || hdr_len_out == 2 || hdr_len_out == 4) {
                    nal_hdr_len = hdr_len_out;
                    cap->_nal_len_size = hdr_len_out;
                }
                if (is_sync) {
                    for (size_t i = 0; i < ps_count; i++) {
                        const uint8_t *ps = NULL;
                        size_t ps_size = 0;
                        if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, i, &ps, &ps_size, NULL, NULL) == noErr &&
                            ps && ps_size > 0) {
                            uint8_t ntype = ps[0] & 0x1f;
                            if (ntype == 7 || ntype == 8) {
                                [cap->_annexb appendBytes:start_code length:4];
                                [cap->_annexb appendBytes:ps length:ps_size];
                            }
                        }
                    }
                }
            }
        }

        CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sampleBuffer);
        if (block) {
            size_t len_at_off = 0;
            size_t total_len = 0;
            char *ptr = NULL;
            NSMutableData *tmp_block = nil;
            OSStatus bstat = CMBlockBufferGetDataPointer(block, 0, &len_at_off, &total_len, &ptr);
            if (bstat == kCMBlockBufferNoErr && total_len > 0) {
                const uint8_t *data = (const uint8_t *)ptr;
                if (len_at_off < total_len) {
                    tmp_block = [NSMutableData dataWithLength:total_len];
                    if (CMBlockBufferCopyDataBytes(block, 0, total_len, tmp_block.mutableBytes) == kCMBlockBufferNoErr) {
                        data = (const uint8_t *)tmp_block.bytes;
                    } else {
                        total_len = 0;
                    }
                }
                size_t off = 0;
                size_t hdr = (size_t)nal_hdr_len;
                while (off + hdr <= total_len) {
                    uint32_t nal_len = 0;
                    for (size_t k = 0; k < hdr; k++) {
                        nal_len = (nal_len << 8) | data[off + k];
                    }
                    off += hdr;
                    if (nal_len == 0 || off + nal_len > total_len) {
                        break;
                    }
                    uint8_t ntype = data[off] & 0x1f;
                    if (ntype == 1 || ntype == 5 || ntype == 7 || ntype == 8) {
                        [cap->_annexb appendBytes:start_code length:4];
                        [cap->_annexb appendBytes:data + off length:nal_len];
                    }
                    off += nal_len;
                }
            }
        }

        if (cap->_annexb.length > 0) {
            if (write_all_fd(cap, (const uint8_t *)cap->_annexb.bytes, cap->_annexb.length) != 0) {
                if (errno == EPIPE || errno == EBADF) {
                    atomic_store(&cap->_stopped, true);
                    if (cap->_write_fd >= 0) {
                        close(cap->_write_fd);
                        cap->_write_fd = -1;
                    }
                }
            }
        }
        pthread_mutex_unlock(&cap->_write_lock);
    }
}

@implementation TintaCapture

- (instancetype)initWithFd:(int)fd {
    self = [super init];
    if (self) {
        pthread_mutex_init(&_encode_lock, NULL);
        pthread_mutex_init(&_write_lock, NULL);
        atomic_init(&_stopped, false);
        _write_fd = fd;
        _nal_len_size = 4;
        _vt = NULL;
        _annexb = [[NSMutableData alloc] initWithCapacity:128 * 1024];
        _frame_idx = 0;
        _last_pts = kCMTimeInvalid;
        _force_keyframe = YES;
    }
    return self;
}

- (void)dealloc {
    if (_vt) {
        VTCompressionSessionInvalidate(_vt);
        CFRelease(_vt);
        _vt = NULL;
    }
    if (_write_fd >= 0) {
        close(_write_fd);
        _write_fd = -1;
    }
    pthread_mutex_destroy(&_encode_lock);
    pthread_mutex_destroy(&_write_lock);
}

- (void)stream:(SCStream *)stream didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeScreen || !sampleBuffer || atomic_load(&_stopped)) {
        return;
    }
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, false);
    if (attachments && CFArrayGetCount(attachments) > 0) {
        CFDictionaryRef dict = (CFDictionaryRef)CFArrayGetValueAtIndex(attachments, 0);
        CFTypeRef val = CFDictionaryGetValue(dict, (__bridge CFStringRef)SCStreamFrameInfoStatus);
        if (val) {
            NSInteger status = [(__bridge NSNumber *)val integerValue];
            if (status != SCFrameStatusComplete && status != SCFrameStatusStarted) {
                return;
            }
        }
    }
    CVImageBufferRef img = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!img) {
        return;
    }

    pthread_mutex_lock(&_encode_lock);
    if (atomic_load(&_stopped) || !_vt) {
        pthread_mutex_unlock(&_encode_lock);
        return;
    }
    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
    if (!CMTIME_IS_NUMERIC(pts) || (_frame_idx > 0 && CMTimeCompare(pts, _last_pts) <= 0)) {
        pts = (_frame_idx > 0 && CMTIME_IS_NUMERIC(_last_pts))
            ? CMTimeAdd(_last_pts, CMTimeMake(1, 1000))
            : CMTimeMake(0, 1000);
    }
    _last_pts = pts;
    _frame_idx++;

    NSDictionary *frameProps = nil;
    if (_force_keyframe) {
        frameProps = @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame: @YES};
        _force_keyframe = NO;
    }
    VTCompressionSessionEncodeFrame(
        _vt,
        img,
        pts,
        kCMTimeInvalid,
        (__bridge CFDictionaryRef)frameProps,
        NULL,
        NULL
    );
    pthread_mutex_unlock(&_encode_lock);
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    atomic_store(&_stopped, true);
    pthread_mutex_lock(&_write_lock);
    if (_write_fd >= 0) {
        close(_write_fd);
        _write_fd = -1;
    }
    pthread_mutex_unlock(&_write_lock);
}

@end

static SCShareableContent *fetch_shareable_content(NSError * __autoreleasing *out_err) {
    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    __block SCShareableContent *got = nil;
    __block NSError *err = nil;
    [SCShareableContent getShareableContentWithCompletionHandler:^(SCShareableContent *content, NSError *e) {
        got = content;
        err = e;
        dispatch_semaphore_signal(sem);
    }];
    dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
    if (out_err) {
        *out_err = err;
    }
    return got;
}

static uint32_t clamp_even_dim(double v) {
    uint32_t d = ((uint32_t)llround(v)) & ~1U;
    if (d < 320) d = 320;
    if (d > 4096) d = 4096;
    return d;
}

static void *start_sc_capture(
    SCContentFilter *filter,
    uint32_t enc_w,
    uint32_t enc_h,
    int write_fd,
    char **err_out
) {
    configure_pipe_write_fd(write_fd);

    TintaCapture *cap = [[TintaCapture alloc] initWithFd:write_fd];
    OSStatus vt_err = VTCompressionSessionCreate(
        NULL,
        (int32_t)enc_w,
        (int32_t)enc_h,
        kCMVideoCodecType_H264,
        NULL,
        NULL,
        NULL,
        vt_output_callback,
        (__bridge void *)cap,
        &cap->_vt
    );
    if (vt_err != noErr || !cap->_vt) {
        close(write_fd);
        cap->_write_fd = -1;
        set_err(err_out, "VTCompressionSessionCreate failed");
        return NULL;
    }

    VTSessionSetProperty(cap->_vt, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    VTSessionSetProperty(cap->_vt, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    VTSessionSetProperty(cap->_vt, kVTCompressionPropertyKey_MaxKeyFrameInterval, (__bridge CFNumberRef)@(120));
    VTSessionSetProperty(cap->_vt, kVTCompressionPropertyKey_ExpectedFrameRate, (__bridge CFNumberRef)@(60));
    VTSessionSetProperty(cap->_vt, kVTCompressionPropertyKey_AverageBitRate, (__bridge CFNumberRef)@(8000000));
    VTSessionSetProperty(
        cap->_vt,
        kVTCompressionPropertyKey_DataRateLimits,
        (__bridge CFArrayRef)@[@(1500000), @(1.0)]
    );
    VTSessionSetProperty(
        cap->_vt,
        kVTCompressionPropertyKey_ProfileLevel,
        kVTProfileLevel_H264_High_AutoLevel
    );
    VTCompressionSessionPrepareToEncodeFrames(cap->_vt);

    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
    cfg.width = enc_w;
    cfg.height = enc_h;
    cfg.scalesToFit = YES;
    cfg.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
    cfg.minimumFrameInterval = CMTimeMake(1, 60);
    cfg.showsCursor = YES;
    cfg.queueDepth = 3;

    cap->_sample_queue = dispatch_queue_create("dev.spenmouse.capture", DISPATCH_QUEUE_SERIAL);
    cap->_stream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:cap];

    NSError *add_err = nil;
    if (![cap->_stream addStreamOutput:cap type:SCStreamOutputTypeScreen sampleHandlerQueue:cap->_sample_queue error:&add_err]) {
        set_err(err_out, add_err ? add_err.localizedDescription.UTF8String : "addStreamOutput failed");
        cap->_stream = nil;
        VTCompressionSessionInvalidate(cap->_vt);
        CFRelease(cap->_vt);
        cap->_vt = NULL;
        close(write_fd);
        cap->_write_fd = -1;
        return NULL;
    }

    dispatch_semaphore_t start_sem = dispatch_semaphore_create(0);
    __block NSError *start_err = nil;
    [cap->_stream startCaptureWithCompletionHandler:^(NSError *e) {
        start_err = e;
        dispatch_semaphore_signal(start_sem);
    }];
    long timed_out = dispatch_semaphore_wait(start_sem, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC));
    if (timed_out != 0 || start_err) {
        if (start_err) {
            set_err(err_out, start_err.localizedDescription.UTF8String);
        } else {
            set_err(err_out, "SCStream startCapture timed out");
        }
        atomic_store(&cap->_stopped, true);
        cap->_stream = nil;
        VTCompressionSessionInvalidate(cap->_vt);
        CFRelease(cap->_vt);
        cap->_vt = NULL;
        close(write_fd);
        cap->_write_fd = -1;
        return NULL;
    }

    return (void *)CFBridgingRetain(cap);
}

void *tinta_mac_capture_start(
    uint32_t display_id,
    uint32_t out_w,
    uint32_t out_h,
    int write_fd,
    char **err_out
) {
    @autoreleasepool {
        if (err_out) {
            *err_out = NULL;
        }
        if (NSClassFromString(@"SCStream") == nil) {
            if (write_fd >= 0) {
                close(write_fd);
            }
            set_err(err_out, "ScreenCaptureKit requires macOS 12.3+");
            return NULL;
        }
        if (write_fd < 0) {
            set_err(err_out, "invalid write fd");
            return NULL;
        }

        SCDisplay *found = nil;
        NSError *last_err = nil;
        for (int attempt = 0; attempt < 20; attempt++) {
            NSError *err = nil;
            SCShareableContent *got = fetch_shareable_content(&err);
            if (err) {
                last_err = err;
                break;
            }
            if (got) {
                for (SCDisplay *d in got.displays) {
                    if (d.displayID == display_id) {
                        found = d;
                        break;
                    }
                }
            }
            if (found) {
                break;
            }
            usleep(100 * 1000);
        }

        if (!found) {
            close(write_fd);
            if (last_err) {
                set_err(err_out, last_err.localizedDescription.UTF8String);
            } else {
                set_err(err_out, "display not found in SCShareableContent");
            }
            return NULL;
        }

        uint32_t req_w = clamp_even_dim((double)out_w);
        uint32_t req_h = clamp_even_dim((double)out_h);
        uint32_t enc_w = req_w;
        uint32_t enc_h = req_h;
        uint32_t virt_id = tinta_mac_virtual_id();
        if (virt_id == 0 || display_id != virt_id) {
            size_t disp_w = 0;
            size_t disp_h = 0;
            CGDisplayModeRef mode = CGDisplayCopyDisplayMode(display_id);
            if (mode) {
                disp_w = CGDisplayModeGetPixelWidth(mode);
                disp_h = CGDisplayModeGetPixelHeight(mode);
                CGDisplayModeRelease(mode);
            }
            if (disp_w == 0 || disp_h == 0) {
                disp_w = (size_t)found.width;
                disp_h = (size_t)found.height;
            }
            if (disp_w > 0 && disp_h > 0) {
                double fit = fmin((double)req_w / (double)disp_w, (double)req_h / (double)disp_h);
                if (fit > 1.0) {
                    fit = 1.0;
                }
                enc_w = clamp_even_dim((double)disp_w * fit);
                enc_h = clamp_even_dim((double)disp_h * fit);
            }
        }

        SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:found excludingWindows:@[]];
        return start_sc_capture(filter, enc_w, enc_h, write_fd, err_out);
    }
}

void *tinta_mac_capture_window_start(
    uint32_t wid,
    uint32_t out_w,
    uint32_t out_h,
    int write_fd,
    char **err_out
) {
    @autoreleasepool {
        if (err_out) {
            *err_out = NULL;
        }
        if (NSClassFromString(@"SCStream") == nil) {
            if (write_fd >= 0) {
                close(write_fd);
            }
            set_err(err_out, "ScreenCaptureKit requires macOS 12.3+");
            return NULL;
        }
        if (write_fd < 0) {
            set_err(err_out, "invalid write fd");
            return NULL;
        }
        if (wid == 0) {
            close(write_fd);
            set_err(err_out, "invalid window id");
            return NULL;
        }

        SCWindow *found = nil;
        NSError *last_err = nil;
        for (int attempt = 0; attempt < 10; attempt++) {
            NSError *err = nil;
            SCShareableContent *got = fetch_shareable_content(&err);
            if (err) {
                last_err = err;
                break;
            }
            if (got) {
                for (SCWindow *w in got.windows) {
                    if (w.windowID == wid) {
                        found = w;
                        break;
                    }
                }
            }
            if (found) {
                break;
            }
            usleep(100 * 1000);
        }

        if (!found) {
            close(write_fd);
            if (last_err) {
                set_err(err_out, last_err.localizedDescription.UTF8String);
            } else {
                set_err(err_out, "window not found in SCShareableContent");
            }
            return NULL;
        }

        uint32_t req_w = clamp_even_dim((double)out_w);
        uint32_t req_h = clamp_even_dim((double)out_h);
        uint32_t enc_w = req_w;
        uint32_t enc_h = req_h;

        uint32_t did = tinta_mac_display_for_window(wid);
        double scale = 1.0;
        CGRect disp_bounds = CGDisplayBounds(did);
        CGDisplayModeRef mode = CGDisplayCopyDisplayMode(did);
        if (mode) {
            size_t disp_pixel_w = CGDisplayModeGetPixelWidth(mode);
            CGDisplayModeRelease(mode);
            if (disp_pixel_w > 0 && disp_bounds.size.width > 0.0) {
                scale = (double)disp_pixel_w / disp_bounds.size.width;
            }
        }
        if (!isfinite(scale) || scale < 1.0) scale = 1.0;
        if (scale > 3.0) scale = 3.0;

        double nat_w = found.frame.size.width * scale;
        double nat_h = found.frame.size.height * scale;
        if (nat_w > 0.0 && nat_h > 0.0) {
            double fit = fmin((double)req_w / nat_w, (double)req_h / nat_h);
            if (fit > 1.0) {
                fit = 1.0;
            }
            double w = nat_w * fit;
            double h = nat_h * fit;
            if (w < 320.0 || h < 320.0) {
                double up = fmax(320.0 / w, 320.0 / h);
                double max_up = fmin((double)req_w / w, (double)req_h / h);
                if (max_up > 1.0) {
                    up = fmin(up, max_up);
                    w *= up;
                    h *= up;
                }
            }
            enc_w = clamp_even_dim(w);
            enc_h = clamp_even_dim(h);
        }

        SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:found];
        return start_sc_capture(filter, enc_w, enc_h, write_fd, err_out);
    }
}

void tinta_mac_capture_stop(void *handle) {
    if (!handle) {
        return;
    }
    @autoreleasepool {
        if (NSClassFromString(@"SCStream") != nil) {
            TintaCapture *cap = (__bridge_transfer TintaCapture *)handle;
            atomic_store(&cap->_stopped, true);

            if (cap->_stream) {
                dispatch_semaphore_t sem = dispatch_semaphore_create(0);
                [cap->_stream stopCaptureWithCompletionHandler:^(NSError *e) {
                    dispatch_semaphore_signal(sem);
                }];
                dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
                cap->_stream = nil;
            }

            pthread_mutex_lock(&cap->_encode_lock);
            if (cap->_vt) {
                VTCompressionSessionInvalidate(cap->_vt);
                CFRelease(cap->_vt);
                cap->_vt = NULL;
            }
            pthread_mutex_unlock(&cap->_encode_lock);

            pthread_mutex_lock(&cap->_write_lock);
            if (cap->_write_fd >= 0) {
                close(cap->_write_fd);
                cap->_write_fd = -1;
            }
            pthread_mutex_unlock(&cap->_write_lock);
        }
    }
}

#pragma clang diagnostic pop

/* -------------------------------------------------------------------------- */
/* Workspaces / Window Lending (CGWindowList + AXUIElement)                   */
/* -------------------------------------------------------------------------- */

static NSString *sanitize_field(NSString *s, BOOL strip_commas) {
    if (!s || s.length == 0) {
        return @"";
    }
    NSString *clean = [[s stringByReplacingOccurrencesOfString:@"\t" withString:@" "]
        stringByReplacingOccurrencesOfString:@"\n" withString:@" "];
    clean = [clean stringByReplacingOccurrencesOfString:@"\r" withString:@" "];
    if (strip_commas) {
        clean = [clean stringByReplacingOccurrencesOfString:@"," withString:@" "];
    }
    return clean;
}

static NSString *truncate_chars(NSString *s, NSUInteger max_chars) {
    if (!s || s.length == 0) {
        return @"";
    }
    NSMutableString *out = [NSMutableString string];
    __block NSUInteger count = 0;
    [s enumerateSubstringsInRange:NSMakeRange(0, s.length)
                          options:NSStringEnumerationByComposedCharacterSequences
                       usingBlock:^(NSString *sub, NSRange r, NSRange er, BOOL *stop) {
        if (count >= max_chars) {
            *stop = YES;
            return;
        }
        [out appendString:sub];
        count++;
    }];
    return out;
}

static BOOL csv_contains(const char *csv, NSString *needle) {
    if (!csv || !*csv || !needle || needle.length == 0) {
        return NO;
    }
    const char *n = [needle UTF8String];
    size_t nlen = strlen(n);
    const char *p = csv;
    while (*p) {
        const char *comma = strchr(p, ',');
        size_t tok_len = comma ? (size_t)(comma - p) : strlen(p);
        if (tok_len == nlen && memcmp(p, n, nlen) == 0) {
            return YES;
        }
        if (!comma) {
            break;
        }
        p = comma + 1;
    }
    return NO;
}

static BOOL is_system_chrome_owner(NSString *owner) {
    if (!owner || owner.length == 0) {
        return NO;
    }
    static NSSet<NSString *> *ignored = nil;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        ignored = [NSSet setWithObjects:
            @"Window Server",
            @"WindowManager",
            @"Dock",
            @"Wallpaper",
            @"Fondo de pantalla",
            @"Control Center",
            @"Centro de control",
            @"Notification Center",
            @"Centro de notificaciones",
            @"Spotlight",
            @"SystemUIServer",
            nil];
    });
    return [ignored containsObject:owner];
}

static BOOL extract_window_entry(
    NSDictionary *info,
    pid_t self_pid,
    double min_dim,
    int32_t *out_pid,
    uint32_t *out_wid,
    CGRect *out_wb
) {
    NSNumber *layer = info[(__bridge NSString *)kCGWindowLayer];
    if (!layer || [layer integerValue] != 0) {
        return NO;
    }
    NSNumber *alpha = info[(__bridge NSString *)kCGWindowAlpha];
    if (alpha && [alpha doubleValue] <= 0.0) {
        return NO;
    }
    NSNumber *pidNum = info[(__bridge NSString *)kCGWindowOwnerPID];
    NSNumber *widNum = info[(__bridge NSString *)kCGWindowNumber];
    if (!pidNum || !widNum) {
        return NO;
    }
    int32_t pid = [pidNum intValue];
    uint32_t wid = [widNum unsignedIntValue];
    if (pid <= 0 || wid == 0 || pid == self_pid) {
        return NO;
    }
    NSString *raw_owner = info[(__bridge NSString *)kCGWindowOwnerName];
    if (is_system_chrome_owner(raw_owner)) {
        return NO;
    }
    NSDictionary *bdict = info[(__bridge NSString *)kCGWindowBounds];
    CGRect wb = CGRectZero;
    if (!bdict || !CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)bdict, &wb)) {
        return NO;
    }
    if (wb.size.width < min_dim || wb.size.height < min_dim) {
        return NO;
    }
    if (out_pid) *out_pid = pid;
    if (out_wid) *out_wid = wid;
    if (out_wb) *out_wb = wb;
    return YES;
}

static uint32_t display_for_rect(CGRect r) {
    if (CGRectIsEmpty(r)) {
        return CGMainDisplayID();
    }
    CGPoint mid = CGPointMake(CGRectGetMidX(r), CGRectGetMidY(r));
    CGDirectDisplayID hit = 0;
    uint32_t cnt = 0;
    if (CGGetDisplaysWithPoint(mid, 1, &hit, &cnt) == kCGErrorSuccess && cnt > 0 && hit != 0) {
        return hit;
    }
    CGDirectDisplayID ids[32];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(32, ids, &count) == kCGErrorSuccess && count > 0) {
        uint32_t best = 0;
        double best_area = 0.0;
        for (uint32_t i = 0; i < count; i++) {
            CGRect db = CGDisplayBounds(ids[i]);
            CGRect inter = CGRectIntersection(db, r);
            double area = CGRectIsNull(inter) ? 0.0 : (inter.size.width * inter.size.height);
            if (area > best_area) {
                best_area = area;
                best = ids[i];
            }
        }
        if (best != 0) {
            return best;
        }
    }
    if (CGGetDisplaysWithRect(r, 1, &hit, &cnt) == kCGErrorSuccess && cnt > 0 && hit != 0) {
        return hit;
    }
    return CGMainDisplayID();
}

char *tinta_mac_ws_list(const char *lent_csv) {
    @autoreleasepool {
        const NSUInteger LIST_MAX = 1350;
        const NSUInteger TITLE_MAX = 28;

        CGDirectDisplayID disp_ids[32];
        uint32_t disp_count = 0;
        if (CGGetActiveDisplayList(32, disp_ids, &disp_count) != kCGErrorSuccess || disp_count == 0) {
            return strdup("");
        }

        CGRect disp_bounds[32];
        NSMutableArray<NSMutableArray<NSString *> *> *disp_wins = [NSMutableArray arrayWithCapacity:disp_count];
        for (uint32_t i = 0; i < disp_count; i++) {
            disp_bounds[i] = CGDisplayBounds(disp_ids[i]);
            [disp_wins addObject:[NSMutableArray array]];
        }

        pid_t self_pid = getpid();
        CFArrayRef list = CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID
        );
        if (list) {
            NSArray *arr = (__bridge_transfer NSArray *)list;
            for (NSDictionary *info in arr) {
                int32_t pid = 0;
                uint32_t wid = 0;
                CGRect wb = CGRectZero;
                if (!extract_window_entry(info, self_pid, 50.0, &pid, &wid, &wb)) {
                    continue;
                }

                // Assign window to the display containing its center, or largest overlap.
                uint32_t target_idx = 0;
                BOOL matched = NO;
                CGPoint mid = CGPointMake(CGRectGetMidX(wb), CGRectGetMidY(wb));
                for (uint32_t i = 0; i < disp_count; i++) {
                    if (CGRectContainsPoint(disp_bounds[i], mid)) {
                        target_idx = i;
                        matched = YES;
                        break;
                    }
                }
                if (!matched) {
                    double best_area = -1.0;
                    for (uint32_t i = 0; i < disp_count; i++) {
                        CGRect inter = CGRectIntersection(disp_bounds[i], wb);
                        double area = CGRectIsNull(inter) ? 0.0 : (inter.size.width * inter.size.height);
                        if (area > best_area) {
                            best_area = area;
                            target_idx = i;
                        }
                    }
                }

                NSString *addr = [NSString stringWithFormat:@"%d:%u", pid, wid];
                NSString *mark = csv_contains(lent_csv, addr) ? @"*" : @"";
                NSString *owner = sanitize_field(info[(__bridge NSString *)kCGWindowOwnerName], YES);
                if (owner.length == 0) {
                    owner = @"app";
                }
                NSString *title = truncate_chars(sanitize_field(info[(__bridge NSString *)kCGWindowName], NO), TITLE_MAX);

                NSString *entry = [NSString stringWithFormat:@"%@%@,%d,%d,%d,%d,%@: %@",
                    mark,
                    addr,
                    (int)llround(wb.origin.x),
                    (int)llround(wb.origin.y),
                    (int)llround(wb.size.width),
                    (int)llround(wb.size.height),
                    owner,
                    title
                ];
                [disp_wins[target_idx] addObject:entry];
            }
        }

        NSMutableData *out = [NSMutableData data];
        for (uint32_t i = 0; i < disp_count; i++) {
            NSString *mon = sanitize_field(display_name_ns(disp_ids[i]), NO);
            CGRect b = disp_bounds[i];
            NSMutableString *line = [NSMutableString stringWithFormat:@"%u\t%@\t%d,%d,%d,%d",
                disp_ids[i],
                mon,
                (int)llround(b.origin.x),
                (int)llround(b.origin.y),
                (int)llround(b.size.width),
                (int)llround(b.size.height)
            ];
            for (NSString *w in disp_wins[i]) {
                NSUInteger cand_bytes = [line lengthOfBytesUsingEncoding:NSUTF8StringEncoding]
                    + 1
                    + [w lengthOfBytesUsingEncoding:NSUTF8StringEncoding]
                    + 1;
                if (out.length + cand_bytes > LIST_MAX) {
                    break;
                }
                [line appendString:@"\t"];
                [line appendString:w];
            }
            [line appendString:@"\n"];
            NSData *ldata = [line dataUsingEncoding:NSUTF8StringEncoding];
            if (!ldata || out.length + ldata.length > LIST_MAX) {
                break;
            }
            [out appendData:ldata];
        }

        char *res = (char *)malloc(out.length + 1);
        if (!res) {
            return NULL;
        }
        if (out.length > 0) {
            memcpy(res, out.bytes, out.length);
        }
        res[out.length] = '\0';
        return res;
    }
}

typedef AXError (*AXGetWindowFn)(AXUIElementRef, CGWindowID *);

static AXGetWindowFn resolve_ax_get_window(void) {
    static dispatch_once_t once;
    static AXGetWindowFn fn = NULL;
    dispatch_once(&once, ^{
        fn = (AXGetWindowFn)dlsym(RTLD_DEFAULT, "_AXUIElementGetWindow");
    });
    return fn;
}

static BOOL lookup_cgwindow_info(int32_t wid, CGRect *out_bounds, NSString **out_title) {
    CFArrayRef list = CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow, (CGWindowID)wid);
    if (!list) {
        return NO;
    }
    NSArray *arr = (__bridge_transfer NSArray *)list;
    for (NSDictionary *info in arr) {
        NSNumber *num = info[(__bridge NSString *)kCGWindowNumber];
        if (num && [num intValue] == wid) {
            NSDictionary *bdict = info[(__bridge NSString *)kCGWindowBounds];
            if (bdict && out_bounds) {
                CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)bdict, out_bounds);
            }
            if (out_title) {
                *out_title = info[(__bridge NSString *)kCGWindowName];
            }
            return YES;
        }
    }
    return NO;
}

static AXUIElementRef find_ax_window(AXUIElementRef app, int32_t wid) {
    CFArrayRef windows = NULL;
    if (AXUIElementCopyAttributeValue(app, kAXWindowsAttribute, (CFTypeRef *)&windows) != kAXErrorSuccess || !windows) {
        return NULL;
    }

    AXUIElementRef target = NULL;
    CFIndex count = CFArrayGetCount(windows);
    AXGetWindowFn get_win = resolve_ax_get_window();
    if (get_win) {
        for (CFIndex i = 0; i < count; i++) {
            AXUIElementRef win = (AXUIElementRef)CFArrayGetValueAtIndex(windows, i);
            CGWindowID awid = 0;
            if (get_win(win, &awid) == kAXErrorSuccess && (int32_t)awid == wid) {
                target = win;
                break;
            }
        }
    }

    if (!target) {
        CGRect cg_bounds = CGRectZero;
        NSString *cg_title = nil;
        if (lookup_cgwindow_info(wid, &cg_bounds, &cg_title)) {
            for (CFIndex i = 0; i < count; i++) {
                AXUIElementRef win = (AXUIElementRef)CFArrayGetValueAtIndex(windows, i);
                CFTypeRef pos_val = NULL;
                CFTypeRef size_val = NULL;
                CGPoint p = CGPointZero;
                CGSize s = CGSizeZero;
                BOOL ok_pos = (AXUIElementCopyAttributeValue(win, kAXPositionAttribute, &pos_val) == kAXErrorSuccess && pos_val &&
                    AXValueGetValue((AXValueRef)pos_val, kAXValueTypeCGPoint, &p));
                BOOL ok_size = (AXUIElementCopyAttributeValue(win, kAXSizeAttribute, &size_val) == kAXErrorSuccess && size_val &&
                    AXValueGetValue((AXValueRef)size_val, kAXValueTypeCGSize, &s));
                if (pos_val) CFRelease(pos_val);
                if (size_val) CFRelease(size_val);
                if (ok_pos && ok_size &&
                    fabs(p.x - cg_bounds.origin.x) <= 2.0 &&
                    fabs(p.y - cg_bounds.origin.y) <= 2.0 &&
                    fabs(s.width - cg_bounds.size.width) <= 2.0 &&
                    fabs(s.height - cg_bounds.size.height) <= 2.0) {
                    if (cg_title && cg_title.length > 0) {
                        CFTypeRef tval = NULL;
                        BOOL title_ok = YES;
                        if (AXUIElementCopyAttributeValue(win, kAXTitleAttribute, &tval) == kAXErrorSuccess && tval) {
                            if (CFGetTypeID(tval) == CFStringGetTypeID()) {
                                NSString *atitle = (__bridge NSString *)tval;
                                title_ok = [atitle isEqualToString:cg_title];
                            }
                            CFRelease(tval);
                        }
                        if (!title_ok) {
                            continue;
                        }
                    }
                    target = win;
                    break;
                }
            }
        }
    }

    if (target) {
        CFRetain(target);
    }
    CFRelease(windows);
    return target;
}

static void unminimize_ax_window(AXUIElementRef win) {
    CFTypeRef min_val = NULL;
    if (AXUIElementCopyAttributeValue(win, kAXMinimizedAttribute, &min_val) == kAXErrorSuccess && min_val) {
        if (CFGetTypeID(min_val) == CFBooleanGetTypeID() && CFBooleanGetValue((CFBooleanRef)min_val)) {
            AXUIElementSetAttributeValue(win, kAXMinimizedAttribute, kCFBooleanFalse);
        }
        CFRelease(min_val);
    }
}

static void raise_and_activate_window(AXUIElementRef win, int32_t pid) {
    AXUIElementSetAttributeValue(win, kAXMainAttribute, kCFBooleanTrue);
    AXUIElementPerformAction(win, kAXRaiseAction);
    NSRunningApplication *app_obj = [NSRunningApplication runningApplicationWithProcessIdentifier:(pid_t)pid];
    if (app_obj) {
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
        [app_obj activateWithOptions:(NSApplicationActivateAllWindows | NSApplicationActivateIgnoringOtherApps)];
#pragma clang diagnostic pop
    }
}

static AXError set_ax_point(AXUIElementRef win, CGPoint pt) {
    AXValueRef val = AXValueCreate(kAXValueTypeCGPoint, &pt);
    if (!val) {
        return kAXErrorFailure;
    }
    AXError err = AXUIElementSetAttributeValue(win, kAXPositionAttribute, val);
    CFRelease(val);
    return err;
}

static AXError set_ax_size(AXUIElementRef win, CGSize sz) {
    AXValueRef val = AXValueCreate(kAXValueTypeCGSize, &sz);
    if (!val) {
        return kAXErrorFailure;
    }
    AXError err = AXUIElementSetAttributeValue(win, kAXSizeAttribute, val);
    CFRelease(val);
    return err;
}

int tinta_mac_move_window(
    int32_t pid,
    int32_t wid,
    double x,
    double y,
    double w,
    double h,
    int raise_and_activate,
    double *old_x,
    double *old_y,
    double *old_w,
    double *old_h
) {
    if (pid <= 0 || wid <= 0) {
        return 0;
    }
    if (!AXIsProcessTrusted()) {
        fprintf(stderr, "ws: cannot move window %d:%d (Accessibility permission not granted)\n", pid, wid);
        return 0;
    }
    @autoreleasepool {
        AXUIElementRef app = AXUIElementCreateApplication((pid_t)pid);
        if (!app) {
            return 0;
        }
        AXUIElementRef target = find_ax_window(app, wid);
        if (!target) {
            fprintf(stderr, "ws: window %d:%d not found in AX tree\n", pid, wid);
            CFRelease(app);
            return 0;
        }

        unminimize_ax_window(target);

        CGPoint cur_pos = CGPointZero;
        CGSize cur_size = CGSizeZero;
        CFTypeRef pos_val = NULL;
        if (AXUIElementCopyAttributeValue(target, kAXPositionAttribute, &pos_val) == kAXErrorSuccess && pos_val) {
            AXValueGetValue((AXValueRef)pos_val, kAXValueTypeCGPoint, &cur_pos);
            CFRelease(pos_val);
        }
        CFTypeRef size_val = NULL;
        if (AXUIElementCopyAttributeValue(target, kAXSizeAttribute, &size_val) == kAXErrorSuccess && size_val) {
            AXValueGetValue((AXValueRef)size_val, kAXValueTypeCGSize, &cur_size);
            CFRelease(size_val);
        }
        if (old_x) *old_x = cur_pos.x;
        if (old_y) *old_y = cur_pos.y;
        if (old_w) *old_w = cur_size.width;
        if (old_h) *old_h = cur_size.height;

        // Shrink first if needed so macOS allows crossing onto a smaller display, then move and resize.
        if (w > 0.0 && h > 0.0 && (cur_size.width > w || cur_size.height > h)) {
            set_ax_size(target, CGSizeMake(fmin(cur_size.width, w), fmin(cur_size.height, h)));
        }

        AXError err = set_ax_point(target, CGPointMake(x, y));
        if (w > 0.0 && h > 0.0) {
            set_ax_size(target, CGSizeMake(w, h));
            AXError err2 = set_ax_point(target, CGPointMake(x, y));
            if (err != kAXErrorSuccess || err2 != kAXErrorSuccess) {
                err = (err2 == kAXErrorSuccess) ? kAXErrorSuccess : (err != kAXErrorSuccess ? err : err2);
            }
        }
        if (err != kAXErrorSuccess) {
            fprintf(stderr, "ws: move %d:%d failed (err=%d)\n", pid, wid, (int)err);
        }

        int ok = (err == kAXErrorSuccess) ? 1 : 0;
        if (ok && raise_and_activate) {
            raise_and_activate_window(target, pid);
        }

        CFRelease(target);
        CFRelease(app);
        return ok;
    }
}

int tinta_mac_focus_window(int32_t pid, int32_t wid) {
    if (pid <= 0 || wid <= 0) {
        return 0;
    }
    if (!AXIsProcessTrusted()) {
        fprintf(stderr, "ws: cannot focus window %d:%d (Accessibility permission not granted)\n", pid, wid);
        return 0;
    }
    @autoreleasepool {
        AXUIElementRef app = AXUIElementCreateApplication((pid_t)pid);
        if (!app) {
            return 0;
        }
        AXUIElementRef target = find_ax_window(app, wid);
        if (!target) {
            CFRelease(app);
            return 0;
        }
        unminimize_ax_window(target);
        raise_and_activate_window(target, pid);
        CFRelease(target);
        CFRelease(app);
        return 1;
    }
}

int tinta_mac_frontmost_window(int32_t *out_pid, uint32_t *out_wid) {
    @autoreleasepool {
        pid_t self_pid = getpid();
        CFArrayRef list = CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID
        );
        if (!list) {
            return 0;
        }
        NSArray *arr = (__bridge_transfer NSArray *)list;
        for (NSDictionary *info in arr) {
            int32_t pid = 0;
            uint32_t wid = 0;
            if (extract_window_entry(info, self_pid, 100.0, &pid, &wid, NULL)) {
                if (out_pid) *out_pid = pid;
                if (out_wid) *out_wid = wid;
                return 1;
            }
        }
        return 0;
    }
}

int tinta_mac_window_exists(uint32_t wid) {
    if (wid == 0 || wid > (uint32_t)INT32_MAX) {
        return 0;
    }
    @autoreleasepool {
        CGRect wb = CGRectZero;
        return (lookup_cgwindow_info((int32_t)wid, &wb, NULL) && !CGRectIsEmpty(wb)) ? 1 : 0;
    }
}

int tinta_mac_window_bounds(uint32_t wid, double *x, double *y, double *w, double *h) {
    if (wid == 0 || wid > (uint32_t)INT32_MAX) {
        return 0;
    }
    @autoreleasepool {
        CGRect wb = CGRectZero;
        if (!lookup_cgwindow_info((int32_t)wid, &wb, NULL) || CGRectIsEmpty(wb)) {
            return 0;
        }
        if (x) *x = wb.origin.x;
        if (y) *y = wb.origin.y;
        if (w) *w = wb.size.width;
        if (h) *h = wb.size.height;
        return 1;
    }
}

char *tinta_mac_window_label(uint32_t wid) {
    @autoreleasepool {
        NSString *label = nil;
        if (wid != 0) {
            CFArrayRef list = CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow, (CGWindowID)wid);
            if (list) {
                NSArray *arr = (__bridge_transfer NSArray *)list;
                for (NSDictionary *info in arr) {
                    NSNumber *num = info[(__bridge NSString *)kCGWindowNumber];
                    if (num && [num unsignedIntValue] == wid) {
                        NSString *owner = [sanitize_field(info[(__bridge NSString *)kCGWindowOwnerName], NO)
                            stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                        NSString *title = [sanitize_field(info[(__bridge NSString *)kCGWindowName], NO)
                            stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                        if (owner.length > 0 && title.length > 0) {
                            label = [NSString stringWithFormat:@"%@: %@", owner, title];
                        } else if (owner.length > 0) {
                            label = owner;
                        } else if (title.length > 0) {
                            label = title;
                        }
                        break;
                    }
                }
            }
        }
        if (!label || label.length == 0) {
            label = [NSString stringWithFormat:@"window-%u", wid];
        }
        return strdup([label UTF8String]);
    }
}

uint32_t tinta_mac_display_for_window(uint32_t wid) {
    if (wid == 0 || wid > (uint32_t)INT32_MAX) {
        return CGMainDisplayID();
    }
    @autoreleasepool {
        CGRect wb = CGRectZero;
        if (!lookup_cgwindow_info((int32_t)wid, &wb, NULL) || CGRectIsEmpty(wb)) {
            return CGMainDisplayID();
        }
        return display_for_rect(wb);
    }
}

static pthread_mutex_t g_ax_pid_lock = PTHREAD_MUTEX_INITIALIZER;
static pid_t g_ax_manual_pids[64];
static size_t g_ax_manual_count = 0;
static size_t g_ax_manual_next = 0;

int tinta_mac_focused_is_text_input(void) {
    if (!AXIsProcessTrusted()) {
        return 0;
    }
    @autoreleasepool {
        AXUIElementRef sys = AXUIElementCreateSystemWide();
        if (!sys) {
            return 0;
        }

        pid_t front_pid = 0;
        NSRunningApplication *front_app = [NSWorkspace sharedWorkspace].frontmostApplication;
        if (front_app) {
            front_pid = front_app.processIdentifier;
        }
        if (front_pid <= 0) {
            CFTypeRef fapp_cf = NULL;
            if (AXUIElementCopyAttributeValue(sys, kAXFocusedApplicationAttribute, &fapp_cf) == kAXErrorSuccess &&
                fapp_cf &&
                CFGetTypeID(fapp_cf) == AXUIElementGetTypeID()) {
                AXUIElementGetPid((AXUIElementRef)fapp_cf, &front_pid);
            }
            if (fapp_cf) {
                CFRelease(fapp_cf);
            }
        }

        AXUIElementRef app = NULL;
        if (front_pid > 0) {
            app = AXUIElementCreateApplication(front_pid);
            if (app) {
                BOOL was_cached = NO;
                pthread_mutex_lock(&g_ax_pid_lock);
                for (size_t i = 0; i < g_ax_manual_count; i++) {
                    if (g_ax_manual_pids[i] == front_pid) {
                        was_cached = YES;
                        break;
                    }
                }
                if (!was_cached) {
                    g_ax_manual_pids[g_ax_manual_next] = front_pid;
                    g_ax_manual_next = (g_ax_manual_next + 1) % 64;
                    if (g_ax_manual_count < 64) {
                        g_ax_manual_count++;
                    }
                }
                pthread_mutex_unlock(&g_ax_pid_lock);
                if (!was_cached) {
                    AXUIElementSetAttributeValue(app, CFSTR("AXManualAccessibility"), kCFBooleanTrue);
                    usleep(80 * 1000);
                }
            }
        }

        CFTypeRef focused_cf = NULL;
        BOOL need_app_fallback = NO;
        if (AXUIElementCopyAttributeValue(sys, kAXFocusedUIElementAttribute, &focused_cf) != kAXErrorSuccess ||
            !focused_cf ||
            CFGetTypeID(focused_cf) != AXUIElementGetTypeID()) {
            need_app_fallback = YES;
        } else {
            CFTypeRef tmp_role_cf = NULL;
            if (AXUIElementCopyAttributeValue((AXUIElementRef)focused_cf, kAXRoleAttribute, &tmp_role_cf) == kAXErrorSuccess &&
                tmp_role_cf &&
                CFGetTypeID(tmp_role_cf) == CFStringGetTypeID()) {
                NSString *tmp_role = (__bridge NSString *)tmp_role_cf;
                if ([tmp_role isEqualToString:@"AXWindow"] || [tmp_role isEqualToString:@"AXApplication"]) {
                    need_app_fallback = YES;
                }
            }
            if (tmp_role_cf) {
                CFRelease(tmp_role_cf);
            }
        }
        if (need_app_fallback && app != NULL) {
            CFTypeRef app_focused_cf = NULL;
            if (AXUIElementCopyAttributeValue(app, kAXFocusedUIElementAttribute, &app_focused_cf) == kAXErrorSuccess &&
                app_focused_cf &&
                CFGetTypeID(app_focused_cf) == AXUIElementGetTypeID()) {
                if (focused_cf) {
                    CFRelease(focused_cf);
                }
                focused_cf = app_focused_cf;
            } else if (app_focused_cf) {
                CFRelease(app_focused_cf);
            }
        }
        if (!focused_cf || CFGetTypeID(focused_cf) != AXUIElementGetTypeID()) {
            if (focused_cf) {
                CFRelease(focused_cf);
            }
            if (app) {
                CFRelease(app);
            }
            CFRelease(sys);
            return 0;
        }
        AXUIElementRef focused = (AXUIElementRef)focused_cf;

        CFTypeRef enabled_cf = NULL;
        if (AXUIElementCopyAttributeValue(focused, kAXEnabledAttribute, &enabled_cf) == kAXErrorSuccess &&
            enabled_cf != NULL &&
            (enabled_cf == kCFBooleanFalse ||
             (CFGetTypeID(enabled_cf) == CFBooleanGetTypeID() && !CFBooleanGetValue((CFBooleanRef)enabled_cf)))) {
            CFRelease(enabled_cf);
            CFRelease(focused_cf);
            if (app) {
                CFRelease(app);
            }
            CFRelease(sys);
            return 0;
        }
        if (enabled_cf) {
            CFRelease(enabled_cf);
        }

        CFTypeRef role_cf = NULL;
        NSString *role = nil;
        if (AXUIElementCopyAttributeValue(focused, kAXRoleAttribute, &role_cf) == kAXErrorSuccess &&
            role_cf &&
            CFGetTypeID(role_cf) == CFStringGetTypeID()) {
            role = (__bridge NSString *)role_cf;
        }

        CFTypeRef subrole_cf = NULL;
        NSString *subrole = nil;
        if (AXUIElementCopyAttributeValue(focused, kAXSubroleAttribute, &subrole_cf) == kAXErrorSuccess &&
            subrole_cf &&
            CFGetTypeID(subrole_cf) == CFStringGetTypeID()) {
            subrole = (__bridge NSString *)subrole_cf;
        }

        Boolean val_settable = false;
        if (AXUIElementIsAttributeSettable(focused, kAXValueAttribute, &val_settable) != kAXErrorSuccess) {
            val_settable = false;
        }

        CFTypeRef editable_cf = NULL;
        BOOL is_editable = (AXUIElementCopyAttributeValue(focused, CFSTR("AXEditable"), &editable_cf) == kAXErrorSuccess &&
            editable_cf != NULL &&
            (editable_cf == kCFBooleanTrue ||
             (CFGetTypeID(editable_cf) == CFBooleanGetTypeID() && CFBooleanGetValue((CFBooleanRef)editable_cf))));
        if (editable_cf) {
            CFRelease(editable_cf);
        }

        CFTypeRef sel_cf = NULL;
        BOOL has_sel_range = (AXUIElementCopyAttributeValue(focused, kAXSelectedTextRangeAttribute, &sel_cf) == kAXErrorSuccess &&
            sel_cf != NULL);
        if (sel_cf) {
            CFRelease(sel_cf);
        }

        CFTypeRef ins_cf = NULL;
        BOOL has_insertion_line = (AXUIElementCopyAttributeValue(focused, kAXInsertionPointLineNumberAttribute, &ins_cf) == kAXErrorSuccess &&
            ins_cf != NULL);
        if (ins_cf) {
            CFRelease(ins_cf);
        }

        BOOL text_role = role && (
            [role isEqualToString:@"AXTextField"] ||
            [role isEqualToString:@"AXTextArea"] ||
            [role isEqualToString:@"AXSearchField"] ||
            [role isEqualToString:@"AXComboBox"]
        );
        BOOL role_ok = val_settable ||
            has_sel_range ||
            has_insertion_line ||
            (subrole && (
                [subrole isEqualToString:@"AXSecureTextField"] ||
                [subrole isEqualToString:@"AXSearchField"]
            ));
        BOOL web_area_ok = role && [role isEqualToString:@"AXWebArea"] && (has_sel_range || has_insertion_line);
        int result = (is_editable || (text_role && role_ok) || ((val_settable || has_insertion_line) && has_sel_range) || web_area_ok) ? 1 : 0;

        if (subrole_cf) {
            CFRelease(subrole_cf);
        }
        if (role_cf) {
            CFRelease(role_cf);
        }
        CFRelease(focused_cf);
        if (app) {
            CFRelease(app);
        }
        CFRelease(sys);
        return result;
    }
}

/* -------------------------------------------------------------------------- */
/* Sleep Prevention (IOPMAssertion)                                           */
/* -------------------------------------------------------------------------- */

static pthread_mutex_t g_sleep_lock = PTHREAD_MUTEX_INITIALIZER;
static IOPMAssertionID g_sleep_assertion = kIOPMNullAssertionID;
static int g_sleep_held = 0;

void tinta_mac_prevent_sleep(int enable) {
    pthread_mutex_lock(&g_sleep_lock);
    if (enable != 0) {
        if (!g_sleep_held) {
            IOReturn ret = IOPMAssertionCreateWithName(
                kIOPMAssertionTypePreventUserIdleDisplaySleep,
                kIOPMAssertionLevelOn,
                CFSTR("TINTA video stream active"),
                &g_sleep_assertion
            );
            if (ret == kIOReturnSuccess) {
                g_sleep_held = 1;
            } else {
                g_sleep_assertion = kIOPMNullAssertionID;
            }
        }
    } else {
        if (g_sleep_held) {
            IOPMAssertionRelease(g_sleep_assertion);
            g_sleep_assertion = kIOPMNullAssertionID;
            g_sleep_held = 0;
        }
    }
    pthread_mutex_unlock(&g_sleep_lock);
}

void tinta_mac_free(void *ptr) {
    free(ptr);
}
