use objc2::{Encode, encode::Encoding, msg_send, runtime::AnyClass};
use objc2_foundation::NSObject;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use raw_window_metal::Layer;
use std::ffi::c_void;
use winit::window::Window;

use crate::AppResult;

const NS_WINDOW_COLLECTION_BEHAVIOR_CAN_JOIN_ALL_SPACES: usize = 1 << 0;
const NS_WINDOW_COLLECTION_BEHAVIOR_STATIONARY: usize = 1 << 4;
const NS_WINDOW_COLLECTION_BEHAVIOR_IGNORES_CYCLE: usize = 1 << 6;
const NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY: isize = 1;
const CG_DESKTOP_ICON_WINDOW_LEVEL_KEY: i32 = 18;

#[derive(Clone, Copy)]
#[repr(transparent)]
struct CGColorSpaceRef(*mut c_void);

// SAFETY: CGColorSpaceRef is a transparent wrapper around `CGColorSpaceRef`, whose Objective-C
// type encoding is a pointer to the opaque `CGColorSpace` structure.
unsafe impl Encode for CGColorSpaceRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGColorSpace", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceExtendedLinearDisplayP3: *const c_void;

    fn CGColorSpaceCreateWithName(name: *const c_void) -> CGColorSpaceRef;
    fn CGWindowLevelForKey(key: i32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: *const c_void);
}

pub(crate) struct MetalLayer {
    layer: Layer,
}

pub(crate) fn create_surface(
    instance: &wgpu::Instance,
    window: &Window,
) -> AppResult<(wgpu::Surface<'static>, MetalLayer)> {
    let handle = window.window_handle()?.as_raw();
    let RawWindowHandle::AppKit(handle) = handle else {
        return Err("macOS did not provide an AppKit window handle".into());
    };

    // SAFETY: The handle belongs to `window`, which remains alive in `Renderer`. This call must
    // run on AppKit's main thread; `Renderer::new` is invoked from `ApplicationHandler::resumed`.
    let layer = unsafe { Layer::from_ns_view(handle.ns_view) };
    // SAFETY: `layer` is a valid CAMetalLayer. Both this wrapper and wgpu retain it for at least as
    // long as the returned surface exists.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
            layer.as_ptr().as_ptr(),
        ))?
    };

    Ok((surface, MetalLayer { layer }))
}

pub(crate) fn configure_output(layer: &MetalLayer, hdr: bool) -> AppResult<()> {
    let layer = layer.layer.as_ptr();

    if hdr {
        // SAFETY: The framework constants and function follow Core Graphics' Create Rule.
        let color_space =
            unsafe { CGColorSpaceCreateWithName(kCGColorSpaceExtendedLinearDisplayP3) };
        if color_space.0.is_null() {
            return Err("failed to create the extended-linear Display P3 color space".into());
        }

        // SAFETY: `Layer` guarantees this is a live CAMetalLayer. `setColorspace:` retains the
        // color space, so the Create-owned reference can be released after the message returns.
        unsafe {
            let layer: &NSObject = layer.cast().as_ref();
            let _: () = msg_send![layer, setColorspace: color_space];
            CFRelease(color_space.0);
        }
    } else {
        // Restore the system-managed SDR color space if the float surface is unavailable.
        // SAFETY: `Layer` guarantees this is a live CAMetalLayer.
        unsafe {
            let layer: &NSObject = layer.cast().as_ref();
            let null_color_space = CGColorSpaceRef(std::ptr::null_mut());
            let _: () = msg_send![layer, setColorspace: null_color_space];
        }
    }

    Ok(())
}

pub(crate) fn configure_wallpaper_window(window: &Window) -> AppResult<()> {
    let handle = window.window_handle()?.as_raw();
    let RawWindowHandle::AppKit(handle) = handle else {
        return Err("macOS did not provide an AppKit window handle".into());
    };

    // Place the renderer immediately below Finder's desktop icons. In current macOS window-level
    // ordering this is the desktop level, above the actual wallpaper and below Finder's icons.
    let level = unsafe { CGWindowLevelForKey(CG_DESKTOP_ICON_WINDOW_LEVEL_KEY) } - 1;
    let collection_behavior = NS_WINDOW_COLLECTION_BEHAVIOR_CAN_JOIN_ALL_SPACES
        | NS_WINDOW_COLLECTION_BEHAVIOR_STATIONARY
        | NS_WINDOW_COLLECTION_BEHAVIOR_IGNORES_CYCLE;
    let application_class =
        AnyClass::get(c"NSApplication").ok_or("AppKit did not provide the NSApplication class")?;

    // SAFETY: The raw handle is a valid NSView owned by `window`, and this function runs on the
    // AppKit main thread directly after window creation. Objective-C integer arguments use the
    // platform-sized NSInteger/NSUInteger ABI represented by isize/usize.
    unsafe {
        let application: *mut NSObject = msg_send![application_class, sharedApplication];
        let application = application
            .as_ref()
            .ok_or("AppKit did not provide the shared application")?;
        let _: bool =
            msg_send![application, setActivationPolicy: NS_APPLICATION_ACTIVATION_POLICY_ACCESSORY];

        let view: &NSObject = handle.ns_view.cast().as_ref();
        let ns_window: *mut NSObject = msg_send![view, window];
        let ns_window = ns_window
            .as_ref()
            .ok_or("the AppKit view is not attached to a window")?;

        let _: () = msg_send![ns_window, setLevel: level as isize];
        let _: () = msg_send![ns_window, setCollectionBehavior: collection_behavior];
        let _: () = msg_send![ns_window, setIgnoresMouseEvents: true];
        let _: () = msg_send![ns_window, setCanHide: false];
        let _: () = msg_send![ns_window, setExcludedFromWindowsMenu: true];
        let _: () = msg_send![ns_window, orderFrontRegardless];
    }

    Ok(())
}
