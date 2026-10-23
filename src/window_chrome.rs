use gpui_kit::{
    component::{ActiveTheme, TitleBar},
    *,
};

gpui_kit::actions!(ferrsphen, [ToggleFullscreen]);

#[derive(Clone, Copy)]
enum Platform {
    Mac,
    Windows,
    Linux,
}
fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(target_os = "windows") {
        Platform::Windows
    } else {
        Platform::Linux
    }
}
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("f11", ToggleFullscreen, None)]);
}
pub fn options(title: impl Into<SharedString>) -> WindowOptions {
    options_for(platform(), title.into())
}
fn options_for(platform: Platform, title: SharedString) -> WindowOptions {
    WindowOptions {
        app_owns_titlebar_drag: true,
        titlebar: Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: true,
            traffic_light_position: matches!(platform, Platform::Mac)
                .then_some(point(px(12.), px(20.))),
        }),
        window_decorations: matches!(platform, Platform::Linux)
            .then_some(WindowDecorations::Client),
        window_background: if matches!(platform, Platform::Linux) {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        },
        ..Default::default()
    }
}
pub fn title_bar(cx: &App) -> TitleBar {
    TitleBar::new()
        .h(rems(3.5))
        .pl_0()
        .bg(cx.theme().background)
        .border_color(cx.theme().border)
}
pub fn leading_inset(window: &Window) -> Pixels {
    px(if cfg!(target_os = "macos") && !window.is_fullscreen() {
        88.
    } else {
        12.
    })
}
pub fn surface(content: impl IntoElement, window: &mut Window, cx: &mut App) -> Div {
    let focus = window
        .use_state(cx, |_, cx| cx.focus_handle())
        .read(cx)
        .clone();
    if window.focused(cx).is_none() {
        focus.focus(window, cx);
    }
    div()
        .track_focus(&focus)
        .size_full()
        .flex()
        .flex_col()
        .on_action(|_: &ToggleFullscreen, window, cx| {
            window.toggle_fullscreen();
            cx.stop_propagation();
        })
        .child(content)
}

#[cfg(test)]
mod tests {
    use super::{Platform, options_for};
    use gpui_kit::{WindowBackgroundAppearance, WindowDecorations};

    #[test]
    fn window_options_keep_native_mac_controls_and_request_linux_client_decorations() {
        let mac = options_for(Platform::Mac, "test".into());
        assert!(mac.titlebar.unwrap().traffic_light_position.is_some());
        assert!(mac.window_decorations.is_none());
        let windows = options_for(Platform::Windows, "test".into());
        let titlebar = windows.titlebar.unwrap();
        assert!(titlebar.appears_transparent);
        assert!(titlebar.traffic_light_position.is_none());
        let linux = options_for(Platform::Linux, "test".into());
        assert_eq!(linux.window_decorations, Some(WindowDecorations::Client));
        assert!(matches!(
            linux.window_background,
            WindowBackgroundAppearance::Transparent
        ));
        assert!(linux.titlebar.unwrap().traffic_light_position.is_none());
    }
}
