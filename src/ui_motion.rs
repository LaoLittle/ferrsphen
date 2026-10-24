use gpui_kit::*;
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

pub const EXIT_SPAN: Duration = Duration::from_millis(120);
pub const ENTRANCE_SPAN: Duration = Duration::from_millis(250);
pub const ENTRANCE_ZOOM: f32 = 0.01;

pub fn page_progress(progress: f32) -> f32 {
    gpui_kit::base::motion::Easing::CubicBezier {
        x1: 0.16,
        y1: 1.,
        x2: 0.3,
        y2: 1.,
    }
    .sample(progress)
}

pub const MESSAGE_ARRIVAL_SPAN: Duration = Duration::from_millis(240);
pub const MESSAGE_SEND_SPAN: Duration = Duration::from_millis(420);

pub fn message_flight_bounds(
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    progress: f32,
) -> Bounds<Pixels> {
    let progress = page_progress(((progress - 0.08) / 0.92).clamp(0., 1.));
    Bounds::new(
        from.origin + (to.origin - from.origin) * progress,
        size(
            from.size.width + (to.size.width - from.size.width) * progress,
            from.size.height + (to.size.height - from.size.height) * progress,
        ),
    )
}

type FlightRenderer = Box<dyn FnOnce(Bounds<Pixels>, f32) -> AnyElement>;

/// Paints above the timeline mask while the real row retains its final layout.
pub struct MessageFlight {
    pub source: Bounds<Pixels>,
    pub target: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    pub progress: f32,
    pub render: Option<FlightRenderer>,
}
impl IntoElement for MessageFlight {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for MessageFlight {
    type RequestLayoutState = ();
    type PrepaintState = Option<AnyElement>;
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
        (window.request_layout(Style::default(), [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let target = self.target.get()?;
        let bounds = message_flight_bounds(self.source, target, self.progress);
        let mut child = self.render.take()?(bounds, self.progress);
        child.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
        child.prepaint_at(bounds.origin, window, cx);
        Some(child)
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        child: &mut Option<AnyElement>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(child) = child {
            child.paint(window, cx);
        }
    }
}

#[derive(Default)]
pub struct Bounce {
    pub position: f32,
    velocity: f32,
    travel: f32,
    extent: f32,
    sampled: Option<Instant>,
    input: Option<Instant>,
    tracking: bool,
    spent: bool,
}
impl Bounce {
    fn rubber(&self, travel: f32) -> f32 {
        let extent = self.extent.max(1.);
        travel * 0.55 / (1. + 0.55 * travel.abs() / extent)
    }

    pub fn packet(
        &mut self,
        delta: f32,
        phase: TouchPhase,
        precise: bool,
        viewport_height: f32,
        now: Instant,
        mut scroll: impl FnMut(f32) -> f32,
    ) {
        let paused = self
            .input
            .replace(now)
            .is_none_or(|last| now.saturating_duration_since(last) >= Duration::from_millis(250));
        if paused && !self.tracking {
            self.spent = false;
        }
        if phase == TouchPhase::Started {
            self.extent = (viewport_height * 0.22).clamp(48., 120.);
            let ratio = (self.position.abs() / self.extent).min(0.99);
            self.travel = self.position / (0.55 * (1. - ratio));
            self.velocity = 0.;
            self.tracking = precise;
            self.spent = false;
            self.sampled = Some(now);
        }
        if self.tracking {
            let mut remainder = delta;
            if self.travel != 0. {
                let next = self.travel + delta;
                if self.travel * next <= 0. {
                    remainder = next;
                    self.travel = 0.;
                } else {
                    self.travel = next.clamp(-self.extent * 8., self.extent * 8.);
                    remainder = 0.;
                }
            }
            if remainder != 0. {
                let unused = remainder - scroll(remainder);
                self.travel = (self.travel + unused).clamp(-self.extent * 8., self.extent * 8.);
            }
            self.position = self.rubber(self.travel);
            self.sampled = Some(now);
        } else {
            let unused = delta - scroll(delta);
            if !self.spent && unused.abs() > 0.01 {
                self.extent = (viewport_height * 0.22).clamp(48., 120.);
                self.position = self.rubber(unused.clamp(-self.extent * 8., self.extent * 8.));
                self.velocity = 0.;
                self.sampled = Some(now);
                self.spent = true;
            }
        }
        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.release();
        }
    }

    fn release(&mut self) {
        self.tracking = false;
        self.travel = 0.;
        self.spent |= self.position != 0.;
    }

    pub fn step(&mut self, now: Instant) -> bool {
        if self.position == 0. && self.velocity == 0. {
            return false;
        }
        let elapsed = self
            .sampled
            .replace(now)
            .map_or(0., |last| now.saturating_duration_since(last).as_secs_f32());
        if self.tracking {
            if self.input.is_some_and(|last| {
                now.saturating_duration_since(last) < Duration::from_millis(100)
            }) {
                return true;
            }
            self.release();
        }
        let spring = SpringConfig::new(324., 36., 1.);
        let state = spring.step(
            SpringState {
                position: self.position,
                velocity: self.velocity,
            },
            0.,
            elapsed,
        );
        if spring.is_settled(state, 0., 0.1) {
            self.position = 0.;
            self.velocity = 0.;
            self.sampled = None;
            false
        } else {
            self.position = state.position;
            self.velocity = state.velocity;
            true
        }
    }
}

type WheelHandler = Rc<dyn Fn(&ScrollWheelEvent, &mut Window, &mut App)>;
pub struct TimelineViewport {
    child: AnyElement,
    displacement: Pixels,
    wheel: WheelHandler,
}
impl TimelineViewport {
    pub fn new(
        child: impl IntoElement,
        displacement: Pixels,
        wheel: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            child: child.into_any_element(),
            displacement,
            wheel: Rc::new(wheel),
        }
    }
}
impl IntoElement for TimelineViewport {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for TimelineViewport {
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
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.with_element_offset(point(px(0.), self.displacement), |window| {
                self.child.prepaint(window, cx);
            });
        });
        hitbox
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
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            self.child.paint(window, cx);
        });
        let hitbox = hitbox.clone();
        let wheel = self.wheel.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture && hitbox.should_handle_scroll(window) {
                wheel(event, window, cx);
            }
        });
    }
}

#[cfg(test)]
mod bounce_tests {
    use super::Bounce;
    use gpui_kit::TouchPhase;
    use std::time::{Duration, Instant};

    #[test]
    fn released_edge_settles_even_while_large_momentum_packets_continue() {
        for direction in [-1., 1.] {
            let now = Instant::now();
            let mut bounce = Bounce::default();
            bounce.packet(
                direction * 1_000_000.,
                TouchPhase::Started,
                true,
                600.,
                now,
                |_| 0.,
            );
            assert!(bounce.position.abs() <= 120.);
            bounce.packet(0., TouchPhase::Ended, true, 600., now, |_| 0.);
            let mut previous = bounce.position.abs();
            for tick in 1..=300 {
                let at = now + Duration::from_millis(tick * 16);
                bounce.packet(
                    direction * 10_000.,
                    TouchPhase::Moved,
                    true,
                    600.,
                    at,
                    |_| 0.,
                );
                bounce.step(at);
                assert!(bounce.position.abs() <= previous);
                previous = bounce.position.abs();
                if tick >= 45 {
                    assert_eq!(bounce.position, 0.);
                }
            }
        }
    }

    #[test]
    fn unphased_fling_rebounds_once_instead_of_holding_the_edge() {
        let now = Instant::now();
        let mut bounce = Bounce::default();
        for tick in 0..100 {
            let at = now + Duration::from_millis(tick * 16);
            bounce.packet(200., TouchPhase::Moved, true, 600., at, |_| 0.);
            bounce.step(at);
            if tick > 45 {
                assert_eq!(bounce.position, 0.);
            }
        }
    }

    #[test]
    fn drag_resistance_increases_and_reverse_motion_unwinds_before_scrolling() {
        let now = Instant::now();
        let mut bounce = Bounce::default();
        bounce.packet(40., TouchPhase::Started, true, 600., now, |_| 0.);
        let first = bounce.position;
        bounce.packet(40., TouchPhase::Moved, true, 600., now, |_| 0.);
        assert!(bounce.position - first < first);
        bounce.packet(-60., TouchPhase::Moved, true, 600., now, |_| {
            panic!("still outside the edge")
        });
        assert!(bounce.position > 0. && bounce.position < first);
        let mut scrolled = 0.;
        bounce.packet(-30., TouchPhase::Moved, true, 600., now, |delta| {
            scrolled += delta;
            delta
        });
        assert_eq!(bounce.position, 0.);
        assert_eq!(scrolled, -10.);
    }

    #[test]
    fn a_new_drag_grabs_the_returning_edge_without_a_jump() {
        let now = Instant::now();
        let mut bounce = Bounce::default();
        bounce.packet(200., TouchPhase::Started, true, 600., now, |_| 0.);
        bounce.packet(0., TouchPhase::Ended, true, 600., now, |_| 0.);
        let at = now + Duration::from_millis(80);
        bounce.step(at);
        let before = bounce.position;
        bounce.packet(0., TouchPhase::Started, true, 600., at, |_| 0.);
        assert!((bounce.position - before).abs() < 0.001);
        bounce.packet(20., TouchPhase::Moved, true, 600., at, |_| 0.);
        assert!(bounce.position > before);
    }
}
