use gpui_kit::{
    base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun, TouchHandleLayout},
    component::ActiveTheme,
    *,
};
use std::ops::Range;

pub struct MessageText {
    id: ElementId,
    content: SharedString,
    text: StyledText,
}
impl MessageText {
    pub fn new(id: impl Into<ElementId>, content: impl Into<SharedString>) -> Self {
        let content = content.into();
        Self {
            id: id.into(),
            text: StyledText::new(content.clone()),
            content,
        }
    }
    fn selection_bounds(&self, range: &Range<usize>) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let layout = self.text.layout();
        let start = layout.position_for_index(range.start)?;
        let end = layout.position_for_index(range.end)?;
        let caret = size(px(0.), layout.line_height());
        Some((Bounds::new(start, caret), Bounds::new(end, caret)))
    }
    fn highlight(&self, range: &Range<usize>, color: Hsla, window: &mut Window) {
        let Some((start, end)) = self.selection_bounds(range) else {
            return;
        };
        if start.top() == end.top() {
            window.paint_quad(fill(
                Bounds::from_corners(start.origin, end.bottom_right()),
                color,
            ));
            return;
        }
        let bounds = self.text.layout().bounds();
        let rects = [
            Bounds::from_corners(start.origin, point(bounds.right(), start.bottom())),
            Bounds::from_corners(
                point(bounds.left(), start.bottom()),
                point(bounds.right(), end.top()),
            ),
            Bounds::from_corners(point(bounds.left(), end.top()), end.bottom_right()),
        ];
        for rect in rects {
            if rect.size.width > px(0.) && rect.size.height > px(0.) {
                window.paint_quad(fill(rect, color));
            }
        }
    }
}
impl IntoElement for MessageText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MessageText {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = (Hitbox, TouchHandleLayout, Option<Range<usize>>);
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = window.with_element_state(
            id.expect("message text identity"),
            |retained: Option<TextSelectionHandle>, _| {
                let handle =
                    retained.unwrap_or_else(|| TextSelectionHandle::new(self.content.clone(), cx));
                (handle.clone(), handle)
            },
        );
        let (layout, ()) = self.text.request_layout(id, inspector, window, cx);
        (layout, handle)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text
            .prepaint(id, inspector, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let runs = [TextSelectionRun::new(
            self.content.clone(),
            self.text.layout().clone(),
            bounds,
        )];
        let projection = handle.update_runs(&runs, cx);
        let range = projection.ranges().first().cloned().flatten();
        let mut registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        if let Some((start, end)) = range
            .as_ref()
            .and_then(|range| self.selection_bounds(range))
        {
            registration = registration.with_selection_edges(start, end);
        }
        handle.register(registration, window, cx);
        let handles = handle.prepaint_touch_handles(window, cx);
        (hitbox, handles, range)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(range) = &prepaint.2 {
            self.highlight(range, cx.theme().selection, window);
        }
        self.text
            .paint(id, inspector, bounds, &mut (), &mut (), window, cx);
        handle.paint_touch_handles(&prepaint.1, cx.theme().primary, window, cx);
    }
}
