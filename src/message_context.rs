use gpui_kit::*;
use std::rc::Rc;

pub(crate) type MenuHandler = Rc<dyn Fn(Bounds<Pixels>, bool, &mut Window, &mut App)>;

pub(crate) struct MessageTrigger {
    child: AnyElement,
    open: MenuHandler,
}
impl MessageTrigger {
    pub fn new(child: impl IntoElement, open: MenuHandler) -> Self {
        Self {
            child: child.into_any_element(),
            open,
        }
    }
}
impl IntoElement for MessageTrigger {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MessageTrigger {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.child.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let open = self.open.clone();
        let target = hitbox.clone();
        window.on_mouse_event(move |event: &LongPressEvent, phase, window, cx| {
            // Stationary touch samples must keep the initially selected word intact.
            if phase.capture()
                && event.phase == TouchPhase::Moved
                && bounds.contains(&event.start_position)
                && (event.position - event.start_position).magnitude() <= 8.
                && gpui_kit::base::TextSelection::has_selection(window, cx)
            {
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            if phase.capture()
                && event.phase == TouchPhase::Ended
                && bounds.contains(&event.start_position)
                && gpui_kit::base::TextSelection::is_selectable_at(event.start_position, window, cx)
                && gpui_kit::base::TextSelection::has_selection(window, cx)
            {
                open(bounds, true, window, cx);
                return;
            }
            if phase.bubble()
                && event.phase == TouchPhase::Started
                && !window.default_prevented()
                && target.is_hovered(window)
                && !gpui_kit::base::TextSelection::is_selectable_at(
                    event.start_position,
                    window,
                    cx,
                )
            {
                window.prevent_default();
                open(bounds, true, window, cx);
            }
        });
        let open = self.open.clone();
        let target = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase.bubble() && event.button == MouseButton::Right && target.is_hovered(window) {
                cx.stop_propagation();
                open(bounds, false, window, cx);
            }
        });
        self.child.paint(window, cx);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum RevealAxis {
    Horizontal,
    Vertical,
}
pub(crate) const REVEAL_DURATION: std::time::Duration = std::time::Duration::from_millis(180);

fn reveal_bounds(mut bounds: Bounds<Pixels>, axis: RevealAxis, progress: f32) -> Bounds<Pixels> {
    match axis {
        RevealAxis::Horizontal => bounds.size.width *= progress.clamp(0., 1.),
        RevealAxis::Vertical => bounds.size.height *= progress.clamp(0., 1.),
    }
    bounds
}

/// Keeps popup placement stable and clips both painting and pointer hitboxes.
pub(crate) struct DirectionalReveal {
    child: AnyElement,
    axis: RevealAxis,
    progress: f32,
    touch_selection: bool,
}
impl DirectionalReveal {
    pub fn new(child: impl IntoElement, axis: RevealAxis, progress: f32) -> Self {
        Self {
            child: div().opacity(progress).child(child).into_any_element(),
            axis,
            progress,
            touch_selection: false,
        }
    }
    pub fn touch_selection(mut self, active: bool) -> Self {
        self.touch_selection = active;
        self
    }
}
impl IntoElement for DirectionalReveal {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for DirectionalReveal {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = (self.progress < 1.).then(|| ContentMask {
            bounds: reveal_bounds(bounds, self.axis, self.progress),
        });
        window.with_content_mask(mask, |window| self.child.prepaint(window, cx));
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.touch_selection {
            gpui_kit::base::TextSelection::register_touch_ui(bounds, window, cx);
            gpui_kit::base::TextSelection::close_edit_menu(window, cx);
        }
        let mask = (self.progress < 1.).then(|| ContentMask {
            bounds: reveal_bounds(bounds, self.axis, self.progress),
        });
        window.with_content_mask(mask, |window| self.child.paint(window, cx));
        if self.progress < 1. {
            window.request_animation_frame();
        }
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::{DirectionalReveal, RevealAxis};
    use gpui::{
        AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render, Styled,
        TestAppContext, VisualTestContext, Window, div, point, px,
    };
    use gpui_kit::component::button::Button;
    struct Harness {
        progress: f32,
        clicks: [usize; 4],
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let buttons = (0..4)
                .map(|index| {
                    Button::new(index)
                        .w(px(100.))
                        .h(px(40.))
                        .label(index.to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.clicks[index] += 1;
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>();
            let mut buttons = buttons.into_iter();
            div()
                .flex()
                .flex_col()
                .items_start()
                .child(DirectionalReveal::new(
                    div()
                        .debug_selector(|| "horizontal-reveal".into())
                        .flex()
                        .w(px(200.))
                        .h(px(40.))
                        .child(buttons.next().unwrap())
                        .child(buttons.next().unwrap()),
                    RevealAxis::Horizontal,
                    self.progress,
                ))
                .child(DirectionalReveal::new(
                    div()
                        .debug_selector(|| "vertical-reveal".into())
                        .flex()
                        .flex_col()
                        .w(px(100.))
                        .h(px(80.))
                        .child(buttons.next().unwrap())
                        .child(buttons.next().unwrap()),
                    RevealAxis::Vertical,
                    self.progress,
                ))
        }
    }
    #[gpui::test]
    fn reveal_keeps_full_layout_and_disables_hidden_pointer_targets(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let handle = cx.add_window(|_, _| Harness {
            progress: 0.5,
            clicks: [0; 4],
        });
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        let horizontal = visual.debug_bounds("horizontal-reveal").unwrap();
        let vertical = visual.debug_bounds("vertical-reveal").unwrap();
        for position in [
            point(px(50.), px(20.)),
            point(px(150.), px(20.)),
            point(px(50.), px(60.)),
            point(px(50.), px(100.)),
        ] {
            visual.simulate_click(position, Default::default());
        }
        cx.update(|cx| {
            handle
                .update(cx, |view, _, cx| {
                    assert_eq!(view.clicks, [1, 0, 1, 0]);
                    view.progress = 1.;
                    cx.notify();
                })
                .unwrap()
        });
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            visual.debug_bounds("horizontal-reveal").unwrap(),
            horizontal
        );
        assert_eq!(visual.debug_bounds("vertical-reveal").unwrap(), vertical);
        visual.simulate_click(point(px(150.), px(20.)), Default::default());
        visual.simulate_click(point(px(50.), px(100.)), Default::default());
        cx.update(|cx| assert_eq!(handle.read(cx).unwrap().clicks, [1, 1, 1, 1]));
    }
}
