use gpui_kit::{
    base::{
        TextSelection, TextSelectionCoverage, TextSelectionEvent, TextSelectionHandle,
        TextSelectionRegistration, TextSelectionRun, TextSelectionSnapshot, TouchHandleLayout,
    },
    component::ActiveTheme,
    *,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

#[derive(Default)]
struct Projection {
    content: SharedString,
    layout: Option<TextLayout>,
    range: Option<Range<usize>>,
}
impl Projection {
    fn update(&mut self, snapshot: Option<TextSelectionSnapshot>, participant: EntityId) -> bool {
        let range = self
            .layout
            .as_ref()
            .and_then(|layout| project_range(snapshot, participant, &self.content, layout));
        let changed = self.range != range;
        self.range = range;
        changed
    }
}

pub struct SelectionState {
    handle: TextSelectionHandle,
    projection: Rc<RefCell<Projection>>,
    registered_bounds: Cell<Option<Bounds<Pixels>>>,
    retained_text: RefCell<Option<StyledText>>,
    _subscription: Subscription,
}
impl SelectionState {
    fn new(content: SharedString, owner: EntityId, cx: &mut App) -> Self {
        let handle = TextSelectionHandle::new(content.clone(), cx);
        let participant = handle.entity_id();
        let projection = Rc::new(RefCell::new(Projection {
            content,
            ..Default::default()
        }));
        let copied = projection.clone();
        handle.copy_with(
            move |_| {
                let projection = copied.borrow();
                projection
                    .range
                    .as_ref()
                    .and_then(|range| projection.content.get(range.clone()))
                    .unwrap_or_default()
                    .to_owned()
            },
            cx,
        );
        let changed = projection.clone();
        let subscription = handle.subscribe(
            move |event, cx| {
                let redraw = match event {
                    TextSelectionEvent::SelectionChanged(snapshot) => {
                        changed.borrow_mut().update(*snapshot, participant)
                    }
                    TextSelectionEvent::Cleared => changed.borrow_mut().update(None, participant),
                    TextSelectionEvent::TouchSelectionChanged => true,
                    TextSelectionEvent::AutoScroll(_) => false,
                };
                if redraw {
                    cx.notify(owner);
                }
            },
            cx,
        );
        Self {
            handle,
            projection,
            registered_bounds: Cell::new(None),
            retained_text: RefCell::new(None),
            _subscription: subscription,
        }
    }
}

fn project_range(
    snapshot: Option<TextSelectionSnapshot>,
    participant: EntityId,
    content: &str,
    layout: &TextLayout,
) -> Option<Range<usize>> {
    let snapshot = snapshot?;
    let points = snapshot.window_points()?;
    let index = |position| {
        let mut index = layout
            .index_for_position(position)
            .unwrap_or_else(|index| index)
            .min(content.len());
        while !content.is_char_boundary(index) {
            index -= 1;
        }
        index
    };
    let (start, end) = match snapshot.coverage() {
        TextSelectionCoverage::Full => (0, content.len()),
        TextSelectionCoverage::Bounded => {
            let anchor = index(points.anchor());
            let cursor = index(points.cursor());
            (anchor.min(cursor), anchor.max(cursor))
        }
        coverage => {
            let point = if snapshot.anchor().entity_id() == Some(participant) {
                points.anchor()
            } else {
                points.cursor()
            };
            let endpoint = index(point);
            if coverage == TextSelectionCoverage::FromStart {
                (0, endpoint)
            } else {
                (endpoint, content.len())
            }
        }
    };
    (start < end).then_some(start..end)
}

fn document_order(origin: Point<Pixels>) -> u64 {
    let ordered = |value: Pixels| {
        let value = f32::from(value);
        if value >= 0. {
            value.to_bits() | (1 << 31)
        } else {
            !value.to_bits()
        }
    };
    (u64::from(ordered(origin.y)) << 32) | u64::from(ordered(origin.x))
}

#[cfg(all(test, feature = "ui-tests"))]
type SelectionProbe = Rc<RefCell<Option<Rc<SelectionState>>>>;

pub struct MessageText {
    #[cfg(all(test, feature = "ui-tests"))]
    probe: Option<SelectionProbe>,
    id: ElementId,
    content: SharedString,
    text: StyledText,
}
impl MessageText {
    pub fn new(id: impl Into<ElementId>, content: impl Into<SharedString>) -> Self {
        let content = content.into();
        Self {
            #[cfg(all(test, feature = "ui-tests"))]
            probe: None,
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
    type RequestLayoutState = Rc<SelectionState>;
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
        let owner = window.current_view();
        let state = window.with_element_state(
            id.expect("message text identity"),
            |retained: Option<Rc<SelectionState>>, _| {
                let state = retained.unwrap_or_else(|| {
                    Rc::new(SelectionState::new(self.content.clone(), owner, cx))
                });
                if state.projection.borrow().content != self.content {
                    state
                        .handle
                        .set_fallback_copy_text(self.content.clone(), cx);
                    state.projection.borrow_mut().content = self.content.clone();
                    state.registered_bounds.set(None);
                    state.retained_text.borrow_mut().take();
                }
                (state.clone(), state)
            },
        );
        if let Some(text) = state.retained_text.borrow_mut().take() {
            self.text = text;
        }
        #[cfg(all(test, feature = "ui-tests"))]
        if let Some(probe) = &self.probe {
            *probe.borrow_mut() = Some(state.clone());
        }
        let (layout, ()) = self.text.request_layout(id, inspector, window, cx);
        (layout, state)
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
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
        let handle = &state.handle;
        let snapshot = handle.snapshot(cx);
        let touch = TextSelection::touch_selection(window, cx).is_some();
        let range = if touch {
            handle
                .update_runs(&runs, cx)
                .ranges()
                .first()
                .cloned()
                .flatten()
        } else {
            // Keep native runs for word/line hit testing, but avoid its per-character
            // projection during desktop drags. Only the two endpoints need hit testing.
            if !snapshot.is_some_and(|snapshot| snapshot.is_selecting())
                && state.registered_bounds.get() != Some(bounds)
            {
                handle.update_runs(&runs, cx);
                state.registered_bounds.set(Some(bounds));
            }
            project_range(
                snapshot,
                handle.entity_id(),
                &self.content,
                self.text.layout(),
            )
        };
        {
            let mut projection = state.projection.borrow_mut();
            projection.layout = Some(self.text.layout().clone());
            projection.range = range.clone();
        }
        let mut registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(document_order(bounds.origin))
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
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(range) = &prepaint.2 {
            self.highlight(range, cx.theme().selection, window);
        }
        self.text
            .paint(id, inspector, bounds, &mut (), &mut (), window, cx);
        state
            .handle
            .paint_touch_handles(&prepaint.1, cx.theme().primary, window, cx);
        // Native hit-test runs share this layout, so keep it current across frames.
        *state.retained_text.borrow_mut() =
            Some(std::mem::replace(&mut self.text, StyledText::new("")));
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::{MessageText, SelectionProbe, SelectionState, project_range};
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit::{
        AnyWindowHandle, AppContext, Context, Entity, InteractiveElement, IntoElement, MouseButton,
        ParentElement, Pixels, Point, Render, SharedString, Styled, Window, WindowOptions,
        base::{TextSelection, TextSelectionRun},
        div, point, px,
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        time::Instant,
    };

    struct Harness {
        texts: Vec<SharedString>,
        probes: Vec<SelectionProbe>,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().flex().flex_col().w(px(280.)).gap_3().children(
                self.texts
                    .iter()
                    .zip(&self.probes)
                    .enumerate()
                    .map(|(index, (text, probe))| {
                        let mut text = MessageText::new("text", text.clone());
                        text.probe = Some(probe.clone());
                        div().id(index).child(text)
                    }),
            )
        }
    }
    fn fixture(
        cx: &mut TestAppContext,
        texts: &[&str],
    ) -> (AnyWindowHandle, Entity<Harness>, Vec<SelectionProbe>) {
        let probes = texts
            .iter()
            .map(|_| Rc::new(RefCell::new(None)))
            .collect::<Vec<_>>();
        let (window, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| Harness {
                    texts: texts
                        .iter()
                        .map(|text| SharedString::from((*text).to_owned()))
                        .collect(),
                    probes: probes.clone(),
                })
            })
            .unwrap()
        });
        draw(cx, window);
        draw(cx, window);
        (window, view, probes)
    }
    fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.run_until_parked();
    }
    fn state(probe: &SelectionProbe) -> Rc<SelectionState> {
        probe.borrow().as_ref().unwrap().clone()
    }
    fn caret(probe: &SelectionProbe, index: usize) -> Point<Pixels> {
        let state = state(probe);
        let projection = state.projection.borrow();
        let layout = projection.layout.as_ref().unwrap();
        layout.position_for_index(index).unwrap() + point(px(0.01), layout.line_height() / 2.)
    }
    fn selected(cx: &mut TestAppContext, window: AnyWindowHandle) -> String {
        cx.update_window(window, |_, window, cx| {
            TextSelection::selected_text(window, cx)
        })
        .unwrap()
    }
    #[gpui::test]
    fn unicode_multiline_drag_copies_latest_range_before_repaint_in_both_directions(
        cx: &mut TestAppContext,
    ) {
        let text = "abcé🙂中文\ndefghijklmnopqrstuvwxyz";
        let (window, _, probes) = fixture(cx, &[text]);
        let start = 3;
        let end = text.find("def").unwrap() + 3;
        for (from, to) in [(start, end), (end, start)] {
            let from = caret(&probes[0], from);
            let to = caret(&probes[0], to);
            let mut visual = VisualTestContext::from_window(window, cx);
            visual.simulate_mouse_down(from, MouseButton::Left, Default::default());
            visual.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
            assert_eq!(selected(cx, window), &text[start..end]);
            visual.simulate_mouse_up(to, MouseButton::Left, Default::default());
            draw(cx, window);
            assert_eq!(selected(cx, window), &text[start..end]);
            cx.update_window(window, |_, window, cx| TextSelection::clear(window, cx))
                .unwrap();
            draw(cx, window);
        }
    }
    #[gpui::test]
    fn selection_spanning_messages_preserves_order_and_complete_middle_message(
        cx: &mut TestAppContext,
    ) {
        let (window, _, probes) = fixture(cx, &["abcdef", "你好🦀", "uvwxyz"]);
        let from = caret(&probes[0], 2);
        let to = caret(&probes[2], 3);
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(from, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        assert_eq!(selected(cx, window), "cdef\n你好🦀\nuvw");
        visual.simulate_mouse_up(to, MouseButton::Left, Default::default());
        draw(cx, window);
        assert_eq!(selected(cx, window), "cdef\n你好🦀\nuvw");
    }
    #[gpui::test]
    fn same_character_moves_do_not_notify_the_chat_again_and_long_text_uses_endpoint_projection(
        cx: &mut TestAppContext,
    ) {
        let text = "abcdefghijklmnopqrstuvwxyz ".repeat(256);
        let (window, view, probes) = fixture(cx, &[&text]);
        let notices = Rc::new(Cell::new(0));
        let observed = notices.clone();
        let _subscription =
            cx.update(|cx| cx.observe(&view, move |_, _| observed.set(observed.get() + 1)));
        let from = caret(&probes[0], 2);
        let to = caret(&probes[0], 16);
        let mut visual = VisualTestContext::from_window(window, cx);
        visual.simulate_mouse_down(from, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(to, Some(MouseButton::Left), Default::default());
        cx.run_until_parked();
        assert_eq!(selected(cx, window), &text[2..16]);
        assert!(notices.get() > 0, "selection must schedule its own repaint");
        let before = notices.get();
        for step in 1..=32 {
            visual.simulate_mouse_move(
                to + point(px(step as f32 * 0.002), px(0.)),
                Some(MouseButton::Left),
                Default::default(),
            );
            cx.run_until_parked();
        }
        assert_eq!(notices.get(), before);
        let participant = state(&probes[0]);
        cx.update(|cx| {
            let snapshot = participant.handle.snapshot(cx);
            let projection = participant.projection.borrow();
            let layout = projection.layout.as_ref().unwrap();
            let started = Instant::now();
            for _ in 0..100 {
                std::hint::black_box(project_range(snapshot, participant.handle.entity_id(), &text, layout));
            }
            let endpoint_time = started.elapsed() / 100;
            let started = Instant::now();
            let native = participant.handle.update_runs(&[TextSelectionRun::new(text.clone(), layout.clone(), layout.bounds())], cx);
            let native_time = started.elapsed();
            assert_eq!(native.ranges()[0].as_ref(), projection.range.as_ref());
            eprintln!("long-message projection: endpoints={endpoint_time:?}, native character walk={native_time:?}");
        });
        visual.simulate_mouse_up(to, MouseButton::Left, Default::default());
    }
}
