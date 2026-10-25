use gpui_kit::*;
use std::rc::Rc;

pub(crate) type MenuHandler = Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)>;

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
                open(bounds, window, cx);
            }
        });
        let open = self.open.clone();
        let target = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase.bubble() && event.button == MouseButton::Right && target.is_hovered(window) {
                cx.stop_propagation();
                open(bounds, window, cx);
            }
        });
        self.child.paint(window, cx);
    }
}
