use gpui_kit::{
    component::{ActiveTheme, TitleBar},
    *,
};

gpui_kit::actions!(ferrsphen, [ToggleFullscreen]);

#[derive(Clone, Copy)]
enum Platform {
    Mac,
    Mobile,
    Windows,
    Linux,
}
fn platform() -> Platform {
    if cfg!(any(target_os = "ios", target_os = "android")) {
        Platform::Mobile
    } else if cfg!(target_os = "macos") {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppWindowKind {
    Login,
    Main,
}
impl AppWindowKind {
    pub fn size(self) -> Size<Pixels> {
        match self {
            Self::Login => size(px(680.), px(480.)),
            Self::Main => size(px(1240.), px(820.)),
        }
    }
    pub fn min_size(self) -> Size<Pixels> {
        match self {
            Self::Login => size(px(640.), px(440.)),
            Self::Main => size(px(900.), px(640.)),
        }
    }
}
pub fn app_options(kind: AppWindowKind, cx: &App) -> WindowOptions {
    if matches!(platform(), Platform::Mobile) {
        return options("ferrsphen");
    }
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(kind.size(), cx)),
        window_min_size: Some(kind.min_size()),
        ..options("ferrsphen")
    }
}
pub fn options(title: impl Into<SharedString>) -> WindowOptions {
    options_for(platform(), title.into())
}
fn options_for(platform: Platform, title: SharedString) -> WindowOptions {
    if matches!(platform, Platform::Mobile) {
        return WindowOptions {
            titlebar: None,
            ..Default::default()
        };
    }
    WindowOptions {
        app_owns_titlebar_drag: true,
        titlebar: Some(TitlebarOptions {
            title: Some(title),
            appears_transparent: true,
            traffic_light_position: matches!(platform, Platform::Mac)
                .then_some(point(px(12.), px(20.))),
        }),
        window_decorations: matches!(platform, Platform::Windows | Platform::Linux)
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
    let visible = window.fully_visible_bounds();
    let viewport = window.viewport_size();
    div()
        .relative()
        .track_focus(&focus)
        .capture_action(|_: &gpui_kit::component::input::Copy, window, cx| {
            let selected = gpui_kit::base::TextSelection::selected_text(window, cx);
            if !selected.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(selected));
                cx.stop_propagation();
            }
        })
        .size_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .pl(visible.left())
        .pt(visible.top())
        .pr((viewport.width - visible.right()).max(px(0.)))
        .pb((viewport.height - visible.bottom()).max(px(0.)))
        .on_action(|_: &ToggleFullscreen, window, cx| {
            window.toggle_fullscreen();
            cx.stop_propagation();
        })
        .child(
            div()
                .debug_selector(|| "safe-content".into())
                // The toolkit's Linux frame already reserves its border and shadow.
                // Fill the parent content box rather than the outer window viewport.
                .flex_1()
                .w_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(content),
        )
}

#[cfg(test)]
mod tests {
    use super::{Platform, options_for};
    use gpui_kit::{WindowBackgroundAppearance, WindowDecorations};

    #[test]
    fn window_options_use_native_mac_controls_and_client_decorations_on_windows_and_linux() {
        let mac = options_for(Platform::Mac, "test".into());
        assert!(mac.titlebar.unwrap().traffic_light_position.is_some());
        assert!(mac.window_decorations.is_none());
        assert!(mac.app_owns_titlebar_drag);
        let windows = options_for(Platform::Windows, "test".into());
        assert_eq!(windows.window_decorations, Some(WindowDecorations::Client));
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
        let mobile = options_for(Platform::Mobile, "test".into());
        assert!(mobile.titlebar.is_none());
        assert!(mobile.window_decorations.is_none());
    }
}

#[cfg(test)]
mod app_window_tests {
    use super::AppWindowKind;
    use gpui_kit::px;
    #[test]
    fn login_is_compact_and_main_minimum_keeps_both_desktop_dimensions_above_phone_breakpoint() {
        let login = AppWindowKind::Login;
        let main = AppWindowKind::Main;
        for kind in [login, main] {
            assert!(kind.size().width >= kind.min_size().width);
            assert!(kind.size().height >= kind.min_size().height);
        }
        assert!(login.size().width < main.min_size().width);
        assert!(login.size().height < main.min_size().height);
        assert!(main.min_size().width >= px(900.));
        assert!(main.min_size().height >= px(640.));
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod layout_tests {
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit::{
        AppContext, Bounds, Context, Edges, InteractiveElement, IntoElement, ParentElement, Pixels,
        Render, Styled, Window, WindowBounds, WindowOptions, div, point, px, size,
    };

    struct FrameProbe {
        insets: Edges<Pixels>,
    }
    impl Render for FrameProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let content = div().relative().size_full().child(
                div()
                    .debug_selector(|| "bottom-right-control".into())
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .size(px(28.)),
            );
            div()
                .size_full()
                .flex()
                .flex_col()
                .pt(self.insets.top)
                .pb(self.insets.bottom)
                .pl(self.insets.left)
                .pr(self.insets.right)
                .child(super::surface(content, window, cx))
        }
    }

    #[gpui::test]
    fn desktop_content_fits_inside_client_frame_at_all_window_sizes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        for dimensions in [
            size(px(1240.), px(820.)),
            size(px(900.), px(640.)),
            size(px(600.), px(720.)),
        ] {
            // Floating, tiled to one edge, maximized: frame space is owned by the parent.
            for insets in [
                Edges::all(px(21.)),
                Edges {
                    top: px(0.),
                    right: px(21.),
                    bottom: px(0.),
                    left: px(0.),
                },
                Edges::all(px(0.)),
            ] {
                let (handle, _) = cx.update(|cx| {
                    gpui_kit::open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                                point(px(0.), px(0.)),
                                dimensions,
                            ))),
                            ..Default::default()
                        },
                        cx,
                        |_, cx| cx.new(|_| FrameProbe { insets }),
                    )
                    .unwrap()
                });
                cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
                    .unwrap();
                let mut visual = VisualTestContext::from_window(handle, cx);
                let content = visual.debug_bounds("safe-content").unwrap();
                let control = visual.debug_bounds("bottom-right-control").unwrap();
                assert_eq!(content.left(), insets.left);
                assert_eq!(content.top(), insets.top);
                assert_eq!(content.right(), dimensions.width - insets.right);
                assert_eq!(content.bottom(), dimensions.height - insets.bottom);
                assert_eq!(control.right(), content.right());
                assert_eq!(control.bottom(), content.bottom());
            }
        }
    }
}
