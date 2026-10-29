use gpui_kit::Application;
use std::sync::atomic::{AtomicBool, Ordering};

static SIMULATED_PHONE: AtomicBool = AtomicBool::new(false);

/// Lays the desktop app out as on a phone (`--phone`). Call before the application starts.
pub fn simulate_phone() {
    SIMULATED_PHONE.store(true, Ordering::Relaxed);
}

/// Desktop process simulating a phone; real mobile targets report `false`.
pub fn simulated_phone() -> bool {
    SIMULATED_PHONE.load(Ordering::Relaxed)
}

/// Mobile UI layout and navigation: real mobile targets or a simulated phone.
pub fn is_mobile() -> bool {
    cfg!(any(target_os = "android", target_os = "ios")) || simulated_phone()
}

pub fn application() -> Application {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        #[cfg(target_os = "ios")]
        let platform = gpui_mobile::current_platform(false);
        #[cfg(target_os = "android")]
        let platform = std::rc::Rc::new(
            gpui_mobile::android::jni::shared_platform().expect("Android platform initialized"),
        );
        Application::with_platform(platform)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        gpui_kit::application()
    }
}

pub fn content_bounds(window: &gpui_kit::Window) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    let visible = window.fully_visible_bounds();
    #[cfg(target_os = "android")]
    if let Ok(insets) = android_system_insets() {
        return inset_bounds(
            visible,
            window.viewport_size(),
            window.scale_factor(),
            insets,
        );
    }
    visible
}

#[cfg(any(target_os = "android", test))]
fn inset_bounds(
    visible: gpui_kit::Bounds<gpui_kit::Pixels>,
    viewport: gpui_kit::Size<gpui_kit::Pixels>,
    scale: f32,
    insets: [i32; 4],
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    use gpui_kit::{Bounds, point, px};
    let [left, top, right, bottom] = insets.map(|value| px(value.max(0) as f32 / scale));
    let left = visible.left().max(left).min(visible.right());
    let top = visible.top().max(top).min(visible.bottom());
    Bounds::from_corners(
        point(left, top),
        point(
            visible.right().min(viewport.width - right).max(left),
            visible.bottom().min(viewport.height - bottom).max(top),
        ),
    )
}

#[cfg(target_os = "android")]
fn android_system_insets() -> Result<[i32; 4], String> {
    use jni::{jni_sig, jni_str, objects::JValue};
    gpui_mobile::android::jni::with_env(|env| {
        let result = (|| -> Result<[i32; 4], jni::errors::Error> {
            let activity = gpui_mobile::android::jni::activity(env)
                .map_err(|_| jni::errors::Error::NullPtr("Activity"))?;
            let window = env
                .call_method(
                    &activity,
                    jni_str!("getWindow"),
                    jni_sig!(() -> android.view.Window),
                    &[],
                )?
                .l()?;
            let decor = env
                .call_method(
                    &window,
                    jni_str!("getDecorView"),
                    jni_sig!(() -> android.view.View),
                    &[],
                )?
                .l()?;
            let insets = env
                .call_method(
                    &decor,
                    jni_str!("getRootWindowInsets"),
                    jni_sig!(() -> android.view.WindowInsets),
                    &[],
                )?
                .l()?;
            if insets.is_null() {
                return Ok([0; 4]);
            }
            // systemBars | displayCutout; IME occlusion is already represented by the viewport.
            if let Ok(edges) = env.call_method(
                &insets,
                jni_str!("getInsets"),
                jni_sig!((int) -> android.graphics.Insets),
                &[JValue::Int(135)],
            ) {
                let edges = edges.l()?;
                return Ok([
                    env.get_field(&edges, jni_str!("left"), jni_sig!(int))?
                        .i()?,
                    env.get_field(&edges, jni_str!("top"), jni_sig!(int))?.i()?,
                    env.get_field(&edges, jni_str!("right"), jni_sig!(int))?
                        .i()?,
                    env.get_field(&edges, jni_str!("bottom"), jni_sig!(int))?
                        .i()?,
                ]);
            }
            env.exception_clear();
            Ok([
                env.call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetLeft"),
                    jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
                env.call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetTop"),
                    jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
                env.call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetRight"),
                    jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
                env.call_method(
                    &insets,
                    jni_str!("getSystemWindowInsetBottom"),
                    jni_sig!(() -> int),
                    &[],
                )?
                .i()?,
            ])
        })();
        if result.is_err() {
            env.exception_clear();
        }
        result.map_err(|error| error.to_string())
    })
}

#[cfg(feature = "desktop")]
pub fn resize_window(
    window: &mut gpui_kit::Window,
    size: gpui_kit::Size<gpui_kit::Pixels>,
    minimum: gpui_kit::Size<gpui_kit::Pixels>,
    cx: &mut gpui_kit::App,
) {
    #[cfg(target_os = "macos")]
    if mac_resize(window, size, minimum, cx) {
        return;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (minimum, cx);
    window.resize(size);
}

#[cfg(all(feature = "desktop", target_os = "macos"))]
fn mac_resize(
    window: &gpui_kit::Window,
    size: gpui_kit::Size<gpui_kit::Pixels>,
    minimum: gpui_kit::Size<gpui_kit::Pixels>,
    cx: &mut gpui_kit::App,
) -> bool {
    use objc2::{msg_send, rc::Retained, runtime::AnyObject};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return false;
    };
    // GPUI owns the view. Retain its window until the queued main-thread resize completes.
    let native: Option<Retained<AnyObject>> = unsafe {
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        let native: *mut AnyObject = msg_send![view, window];
        Retained::retain(native)
    };
    let Some(native) = native else {
        return false;
    };
    // Native resize notifications can enter GPUI. Run after the current entity/window update.
    cx.foreground_executor()
        .spawn(async move {
            unsafe {
                let visible: bool = msg_send![&*native, isVisible];
                if !visible {
                    return;
                }
                let old: NSRect = msg_send![&*native, frame];
                let content = NSRect::new(
                    NSPoint::new(0., 0.),
                    NSSize::new(f32::from(size.width) as f64, f32::from(size.height) as f64),
                );
                let mut frame: NSRect = msg_send![&*native, frameRectForContentRect: content];
                frame.origin = NSPoint::new(
                    old.origin.x + (old.size.width - frame.size.width) / 2.,
                    old.origin.y + (old.size.height - frame.size.height) / 2.,
                );
                let screen: *mut AnyObject = msg_send![&*native, screen];
                if !screen.is_null() {
                    let bounds: NSRect = msg_send![screen, visibleFrame];
                    frame.origin.x = frame.origin.x.clamp(
                        bounds.origin.x,
                        (bounds.origin.x + bounds.size.width - frame.size.width)
                            .max(bounds.origin.x),
                    );
                    frame.origin.y = frame.origin.y.clamp(
                        bounds.origin.y,
                        (bounds.origin.y + bounds.size.height - frame.size.height)
                            .max(bounds.origin.y),
                    );
                }
                let min = NSSize::new(
                    f32::from(minimum.width.min(size.width)) as f64,
                    f32::from(minimum.height.min(size.height)) as f64,
                );
                let _: () = msg_send![&*native, setContentMinSize: min];
                let _: () = msg_send![&*native, setFrame: frame, display: true];
            }
        })
        .detach();
    true
}

#[cfg(test)]
mod tests {
    use super::inset_bounds;
    use gpui_kit::{Bounds, point, px, size};
    #[test]
    fn system_insets_merge_with_existing_safe_area_and_keyboard_without_double_padding() {
        let viewport = size(px(448.), px(997.));
        let full = Bounds::new(point(px(0.), px(0.)), viewport);
        let safe = inset_bounds(full, viewport, 3., [0, 159, 0, 72]);
        assert_eq!(safe.top(), px(53.));
        assert_eq!(safe.bottom(), px(973.));
        assert_eq!(inset_bounds(safe, viewport, 3., [0, 159, 0, 72]), safe);
        let keyboard = Bounds::from_corners(safe.origin, point(px(448.), px(600.)));
        assert_eq!(
            inset_bounds(keyboard, viewport, 3., [0, 159, 0, 72]),
            keyboard
        );
        let landscape = inset_bounds(full, viewport, 3., [159, 0, 72, 0]);
        assert_eq!(landscape.left(), px(53.));
        assert_eq!(landscape.right(), px(424.));
    }
}
