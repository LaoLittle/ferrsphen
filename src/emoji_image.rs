use gpui_kit::{prelude::FluentBuilder, *};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct OncePlayback {
    created: Instant,
    started: Option<Instant>,
    pub finished: bool,
}
impl OncePlayback {
    pub fn new(now: Instant) -> Self {
        Self {
            created: now,
            started: None,
            finished: false,
        }
    }
    fn sample(&mut self, delays: &[Duration], now: Instant) -> (usize, bool) {
        if self.finished
            || (self.started.is_none()
                && now.saturating_duration_since(self.created) > Duration::from_secs(10))
        {
            self.finished = true;
            return (0, false);
        }
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_duration_since(started);
        let total: Duration = delays.iter().sum();
        if elapsed >= total {
            self.finished = true;
            return (0, false);
        }
        (frame_at(delays, elapsed), delays.len() > 1)
    }
}
#[derive(Clone)]
pub(crate) enum Playback {
    Loop,
    Still,
    Hover,
    Once(Rc<RefCell<OncePlayback>>),
}
fn frame_at(delays: &[Duration], elapsed: Duration) -> usize {
    let mut end = Duration::ZERO;
    for (index, delay) in delays.iter().enumerate() {
        end += *delay;
        if elapsed < end {
            return index;
        }
    }
    0
}
#[derive(IntoElement)]
pub(crate) struct EmojiImage {
    id: ElementId,
    path: Option<PathBuf>,
    fallback: String,
    poster: Option<PathBuf>,
    edge: Pixels,
    spinner: bool,
    playback: Playback,
}
impl EmojiImage {
    pub fn new(
        id: impl Into<ElementId>,
        path: Option<PathBuf>,
        fallback: String,
        playback: Playback,
    ) -> Self {
        Self {
            id: id.into(),
            path,
            fallback,
            poster: None,
            edge: px(24.),
            spinner: false,
            playback,
        }
    }
    pub fn edge(mut self, edge: Pixels) -> Self {
        self.edge = edge;
        self
    }
    pub fn with_spinner(mut self) -> Self {
        self.spinner = true;
        self
    }
    pub fn with_poster(mut self, poster: Option<PathBuf>) -> Self {
        self.poster = poster;
        self
    }
}
struct LottieAsset;
impl Asset for LottieAsset {
    type Source = PathBuf;
    type Output = Result<Arc<crate::emoji_lottie::Animation>, Arc<anyhow::Error>>;
    fn load(
        path: PathBuf,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        cx.background_executor().spawn(async move {
            (|| -> anyhow::Result<_> {
                let bytes = std::fs::read(path)?;
                Ok(Arc::new(crate::emoji_lottie::Animation::parse(&bytes)?))
            })()
            .map_err(Arc::new)
        })
    }
}
#[derive(Default)]
struct LottieFrame {
    current: Option<((usize, u16), Arc<RenderImage>)>,
    task: Option<Task<()>>,
}
impl LottieFrame {
    fn clear(&mut self, cx: &mut App) {
        self.task = None;
        if let Some((_, image)) = self.current.take() {
            cx.drop_image(image, None);
        }
    }
}
impl RenderOnce for EmojiImage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hover = matches!(self.playback, Playback::Hover).then(|| {
            window.use_keyed_state((self.id.clone(), "emoji-hover"), cx, |_, _| None::<Instant>)
        });
        let owner = window.current_view();
        let poster = self
            .poster
            .and_then(|path| window.use_asset::<ImgResourceLoader>(&Resource::from(path), cx))
            .and_then(Result::ok);
        let vector = self.path.as_ref().is_some_and(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        });
        let lottie = if vector {
            self.path
                .as_ref()
                .and_then(|path| window.use_asset::<LottieAsset>(path, cx))
                .and_then(|result| {
                    result
                        .inspect_err(|error| tracing::warn!(%error, "load Lottie emoji failed"))
                        .ok()
                })
        } else {
            None
        };
        let source_key = SharedString::from(format!("lottie-frame:{:?}", self.path));
        let image = if !vector {
            self.path
                .and_then(|path| window.use_asset::<ImgResourceLoader>(&Resource::from(path), cx))
                .and_then(Result::ok)
                .filter(|image| image.frame_count() > 0)
        } else {
            None
        };
        let delays = lottie.as_ref().map_or_else(
            || {
                image.as_ref().map_or_else(Vec::new, |image| {
                    (0..image.frame_count())
                        .map(|index| {
                            Duration::from(image.delay(index)).max(Duration::from_millis(10))
                        })
                        .collect()
                })
            },
            |animation| animation.delays.clone(),
        );
        let now = cx.background_executor().now();
        let (frame, playing) = if delays.is_empty() || cx.reduce_motion() {
            if cx.reduce_motion()
                && let Playback::Once(playback) = &self.playback
            {
                playback.borrow_mut().finished = true;
            }
            (0, false)
        } else {
            let phase = |start| {
                let total: Duration = delays.iter().sum();
                let elapsed = now.saturating_duration_since(start);
                let phase = Duration::from_secs_f64(elapsed.as_secs_f64() % total.as_secs_f64());
                (frame_at(&delays, phase), delays.len() > 1)
            };
            match self.playback {
                Playback::Still => (0, false),
                Playback::Hover => hover
                    .as_ref()
                    .and_then(|state| *state.read(cx))
                    .map_or((0, false), phase),
                Playback::Once(playback) => playback.borrow_mut().sample(&delays, now),
                Playback::Loop => {
                    let start =
                        window.use_keyed_state((self.id.clone(), "emoji-clock"), cx, |_, _| now);
                    phase(*start.read(cx))
                }
            }
        };
        let lottie_state = lottie.as_ref().map(|_| {
            window.use_keyed_state((self.id.clone(), source_key), cx, |_, cx| {
                cx.on_release(|state: &mut LottieFrame, cx| state.clear(cx))
                    .detach();
                LottieFrame::default()
            })
        });
        if !playing && let Some(state) = &lottie_state {
            state.update(cx, |state, cx| state.clear(cx));
        }
        let image = if !playing && poster.is_some() {
            poster.clone().map(|image| (image, 0))
        } else {
            image.map(|image| {
                let frame = if !playing {
                    let rest = window.use_keyed_state(
                        (
                            self.id.clone(),
                            SharedString::from(format!("emoji-rest:{:?}", image.id)),
                        ),
                        cx,
                        |_, _| rest_frame(&image),
                    );
                    *rest.read(cx)
                } else {
                    frame
                };
                (image, frame)
            })
        };
        let drawable = image.or_else(|| poster.map(|image| (image, 0)));
        let rendered_lottie = lottie_state
            .as_ref()
            .and_then(|state| state.read(cx).current.as_ref().map(|(key, _)| key.0));
        let child = if drawable.is_some() || lottie.is_some() {
            let edge = self.edge;
            div()
                .debug_selector(move || format!("emoji-frame:{frame}"))
                .size_full()
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            let visible = window.is_window_active()
                                && bounds.intersects(&window.content_mask().bounds);
                            let mut drawable = drawable.clone();
                            if playing
                                && visible
                                && let (Some(animation), Some(state)) = (&lottie, &lottie_state)
                            {
                                // Only the displayed frame and one in-flight render are retained.
                                let resolution = (f32::from(edge) * window.scale_factor())
                                    .ceil()
                                    .clamp(24., 192.)
                                    as u16;
                                let requested = (frame, resolution);
                                let rendered = state.update(cx, |state, cx| {
                                    if state.task.is_none()
                                        && state
                                            .current
                                            .as_ref()
                                            .is_none_or(|(key, _)| *key != requested)
                                    {
                                        let animation = animation.clone();
                                        let job = cx.background_executor().spawn(async move {
                                            Arc::new(RenderImage::new(vec![image::Frame::new(
                                                animation.render(frame, resolution),
                                            )]))
                                        });
                                        state.task = Some(cx.spawn(async move |state, cx| {
                                            let image = job.await;
                                            let _ = state.update(cx, |state, cx| {
                                                if let Some((_, previous)) =
                                                    state.current.replace((requested, image))
                                                {
                                                    cx.drop_image(previous, None);
                                                }
                                                state.task = None;
                                                App::notify(cx, owner);
                                            });
                                        }));
                                    }
                                    state.current.as_ref().map(|(_, image)| image.clone())
                                });
                                if let Some(image) = rendered {
                                    drawable = Some((image, 0));
                                }
                            }
                            if let Some((image, image_frame)) = drawable {
                                let fitted =
                                    ObjectFit::Contain.get_bounds(bounds, image.size(image_frame));
                                if let Err(error) = window.paint_image(
                                    bounds,
                                    fitted,
                                    Corners::default(),
                                    image,
                                    image_frame,
                                    false,
                                ) {
                                    tracing::debug!(%error, "paint emoji failed");
                                }
                            }
                            if playing && visible {
                                window.request_animation_frame();
                            }
                        },
                    )
                    .size_full(),
                )
                .into_any_element()
        } else if self.spinner {
            div()
                .child(gpui_kit::component::spinner::Spinner::new())
                .into_any_element()
        } else {
            div()
                .text_size(px(20.))
                .line_height(px(24.))
                .child(self.fallback)
                .into_any_element()
        };
        div()
            .when(vector, |root| {
                root.debug_selector(move || {
                    rendered_lottie.map_or_else(|| "lottie-rest".into(), |_| "lottie-ready".into())
                })
            })
            .id(self.id)
            .size(self.edge)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when_some(hover, |root, state| {
                root.on_hover(move |hovered, _, cx| {
                    state.update(cx, |started, cx| {
                        *started = hovered.then(|| cx.background_executor().now());
                        App::notify(cx, owner);
                    });
                })
            })
            .child(child)
    }
}

// Some animations begin with a transparent reveal. Without a static poster,
// select a visible frame rather than making the resting reaction disappear.
fn rest_frame(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .max_by_key(|&frame| {
            image.as_bytes(frame).map_or(0, |bytes| {
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| u64::from(pixel[3]))
                    .sum()
            })
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{OncePlayback, rest_frame};
    use std::time::{Duration, Instant};
    #[test]
    fn transparent_opening_frame_is_not_used_as_the_static_fallback() {
        let image = gpui_kit::RenderImage::new(vec![
            image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([0, 0, 0, 0]),
            )),
            image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([20, 80, 180, 255]),
            )),
            image::Frame::new(image::RgbaImage::from_pixel(
                4,
                4,
                image::Rgba([0, 0, 0, 0]),
            )),
        ]);
        assert_eq!(rest_frame(&image), 1);
    }
    #[test]
    fn once_finishes_at_rest_and_cannot_replay_after_remount() {
        let now = Instant::now();
        let mut once = OncePlayback::new(now);
        let delays = [Duration::from_millis(80), Duration::from_millis(120)];
        assert_eq!(once.sample(&delays, now), (0, true));
        assert_eq!(
            once.sample(&delays, now + Duration::from_millis(90)),
            (1, true)
        );
        assert_eq!(
            once.sample(&delays, now + Duration::from_millis(200)),
            (0, false)
        );
        assert_eq!(
            once.sample(&delays, now + Duration::from_secs(3)),
            (0, false)
        );
        let mut offscreen = OncePlayback::new(now);
        assert_eq!(
            offscreen.sample(&delays, now + Duration::from_secs(20)),
            (0, false)
        );
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod vector_ui_tests {
    use super::{EmojiImage, Playback};
    use gpui_kit::{
        AppContext, Context, IntoElement, ParentElement, Render, Styled, TestAppContext,
        VisualTestContext, Window, div, point, px,
    };
    use std::{path::PathBuf, time::Duration};
    struct Preview {
        animation: PathBuf,
        poster: PathBuf,
    }
    impl Render for Preview {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().p_8().child(
                EmojiImage::new(
                    "test-vector",
                    Some(self.animation.clone()),
                    "🙂".into(),
                    Playback::Hover,
                )
                .with_poster(Some(self.poster.clone())),
            )
        }
    }
    #[gpui::test]
    fn svg_poster_and_lottie_playback_switch_on_hover_without_retaining_animation_frames(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let animation = directory.path().join("lottie.json");
        let poster = directory.path().join("emoji.svg");
        std::fs::write(&animation, crate::emoji_lottie::tests::moving_circle()).unwrap();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><circle cx="20" cy="20" r="15" fill="#ff3300"/></svg>"##;
        std::fs::write(&poster, svg).unwrap();
        cx.update(|cx| {
            let rendered = cx.svg_renderer().render_single_frame(svg, 2.).unwrap();
            let edge = rendered.size(0).width.0 as usize;
            assert!(edge >= 80);
            let pixel = &rendered.as_bytes(0).unwrap()[(edge / 2 * edge + edge / 2) * 4..][..4];
            assert_eq!(pixel, &[0, 51, 255, 255]);
        });
        let handle = cx.add_window(|_, _| Preview { animation, poster });
        cx.update_window(handle.into(), |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        let frames = |cx: &mut TestAppContext, count| {
            for _ in 0..count {
                cx.background_executor
                    .advance_clock(Duration::from_millis(16));
                cx.update_window(handle.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                    window.draw(cx).clear(cx);
                })
                .unwrap();
                cx.run_until_parked();
            }
        };
        frames(cx, 4);
        assert!(visual.debug_bounds("lottie-rest").is_some());
        let target = visual.debug_bounds("emoji-frame:0").unwrap().center();
        visual.simulate_mouse_move(target, None, Default::default());
        frames(cx, 12);
        assert!(
            visual.debug_bounds("lottie-ready").is_some(),
            "background Lottie frame should reach the canvas"
        );
        visual.simulate_mouse_move(point(px(1.), px(1.)), None, Default::default());
        frames(cx, 4);
        assert!(visual.debug_bounds("lottie-rest").is_some());
        assert!(visual.debug_bounds("lottie-ready").is_none());
    }
}
