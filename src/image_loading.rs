use gpui_kit::{component::ActiveTheme, prelude::FluentBuilder, *};
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub const REVEAL_DURATION: Duration = Duration::from_millis(480);
const PLACEHOLDER_FPS: f32 = 24.;

#[derive(Clone)]
struct BlurSource(Arc<RenderImage>);
impl Hash for BlurSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}
struct BlurAsset;
impl Asset for BlurAsset {
    type Source = BlurSource;
    type Output = Arc<Vec<Arc<RenderImage>>>;
    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        cx.background_executor()
            .spawn(async move { Arc::new(blur_frames(&source.0)) })
    }
}
fn blur_frames(image: &RenderImage) -> Vec<Arc<RenderImage>> {
    let dimensions = image.size(0);
    let Some(bytes) = image.as_bytes(0) else {
        return Vec::new();
    };
    // RenderImage uses BGRA; imageops treats the channels independently.
    let Some(buffer) = image::RgbaImage::from_raw(
        dimensions.width.0.max(0) as u32,
        dimensions.height.0.max(0) as u32,
        bytes.to_vec(),
    ) else {
        return Vec::new();
    };
    let small = image::DynamicImage::ImageRgba8(buffer)
        .thumbnail(96, 96)
        .into_rgba8();
    [9., 5., 2., 0.6]
        .into_iter()
        .map(|radius| {
            let mut frame = image::imageops::blur(&small, radius);
            let (width, height) = frame.dimensions();
            let feather = (radius * 1.8).min(width.min(height) as f32 * 0.24).max(1.);
            for (x, y, pixel) in frame.enumerate_pixels_mut() {
                let edge = x.min(width - 1 - x).min(y.min(height - 1 - y)) as f32;
                pixel[3] = (pixel[3] as f32 * smooth((edge / feather).min(1.))) as u8;
            }
            Arc::new(RenderImage::new(vec![image::Frame::new(frame)]))
        })
        .collect()
}

fn cloud_pixels(seed: u64, dark: bool, time: f32) -> image::RgbaImage {
    let base_hue = random(seed, 30);
    let patches = (0..9)
        .map(|index| {
            let phase = random(seed, index + 40) * std::f32::consts::TAU;
            let x = (index % 3) as f32 * 0.5
                + (random(seed, index + 50) - 0.5) * 0.18
                + (time * 0.23 + phase).sin() * 0.12;
            let y = (index / 3) as f32 * 0.5
                + (random(seed, index + 60) - 0.5) * 0.18
                + (time * 0.19 + phase).cos() * 0.12;
            let radius = 0.18 + random(seed, index + 70) * 0.18;
            let hue = (base_hue
                + (random(seed, index + 80) - 0.5) * 0.24
                + (time * 0.21 + phase).sin() * 0.09)
                .rem_euclid(1.);
            let lightness = if dark {
                0.065 + random(seed, index + 90) * 0.23
            } else {
                0.73 + random(seed, index + 90) * 0.17
            };
            let color = Rgba::from(hsla(
                hue,
                0.22 + random(seed, index + 100) * 0.2,
                (lightness + (time * 0.32 + phase).sin() * 0.035).clamp(0., 1.),
                1.,
            ));
            (x, y, radius, color)
        })
        .collect::<Vec<_>>();
    let pixels = image::RgbaImage::from_fn(48, 48, |x, y| {
        let u = x as f32 / 47.;
        let v = y as f32 / 47.;
        let u = u + (v * 5. + time * 0.15).sin() * 0.05;
        let v = v + (u * 4. - time * 0.17).cos() * 0.05;
        let mut total = 0.;
        let mut channels = [0.; 3];
        for &(cx, cy, radius, color) in &patches {
            let distance = ((u - cx).powi(2) + (v - cy).powi(2)) / radius.powi(2);
            let weight = (-distance * 2.).exp() + 0.0002;
            total += weight;
            channels[0] += color.b * weight;
            channels[1] += color.g * weight;
            channels[2] += color.r * weight;
        }
        image::Rgba([
            (channels[0] / total * 255.).round() as u8,
            (channels[1] / total * 255.).round() as u8,
            (channels[2] / total * 255.).round() as u8,
            255,
        ])
    });
    image::imageops::blur(&pixels, 2.8)
}

struct LoadingState {
    started: Instant,
    ready: Option<Instant>,
    painted_loading: bool,
    blur: Option<Arc<Vec<Arc<RenderImage>>>>,
    blur_source: Option<BlurSource>,
    path: Option<PathBuf>,
    cloud: Option<Arc<RenderImage>>,
    cloud_sample: Option<(u64, bool, u64)>,
    cloud_task: Option<Task<()>>,
    rotation_task: Option<Task<()>>,
    rotated: Option<Arc<RenderImage>>,
    prepared: Option<Arc<image::RgbaImage>>,
    rotated_angle: f32,
}
impl LoadingState {
    fn new(path: Option<PathBuf>, now: Instant) -> Self {
        Self {
            started: now,
            ready: None,
            painted_loading: false,
            blur: None,
            blur_source: None,
            path,
            cloud: None,
            cloud_sample: None,
            cloud_task: None,
            rotation_task: None,
            rotated: None,
            prepared: None,
            rotated_angle: f32::NAN,
        }
    }
    fn clear_rotation(&mut self, cx: &mut App) {
        self.rotation_task.take();
        self.prepared = None;
        self.rotated_angle = f32::NAN;
        if let Some(image) = self.rotated.take() {
            cx.drop_image(image, None);
        }
    }
    fn clear_blur(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        self.cloud_task = None;
        self.cloud_sample = None;
        if let Some(image) = self.cloud.take() {
            cx.drop_image(image, window.as_deref_mut());
        }
        if let Some(source) = self.blur_source.take() {
            cx.remove_asset::<BlurAsset>(&source);
        }
        if let Some(frames) = self.blur.take() {
            let mut window = window;
            for frame in frames.iter() {
                cx.drop_image(frame.clone(), window.as_deref_mut());
            }
        }
    }
}
fn progress(ready: Option<Instant>, now: Instant) -> f32 {
    ready.map_or(0., |ready| {
        (now.saturating_duration_since(ready).as_secs_f32() / REVEAL_DURATION.as_secs_f32())
            .clamp(0., 1.)
    })
}
fn smooth(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}
fn random(seed: u64, index: usize) -> f32 {
    let mut value = seed.wrapping_add((index as u64).wrapping_mul(0x9e3779b97f4a7c15));
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    ((value ^ (value >> 31)) >> 40) as f32 / (1u32 << 24) as f32
}

/// A stable placeholder and reveal transition sharing GPUI's image/GIF cache.
pub struct LoadingImage {
    id: ElementId,
    seed: u64,
    path: Option<PathBuf>,
    style: StyleRefinement,
    rotation: Option<(f32, f32, f32)>,
    maximum: Option<Size<Pixels>>,
    failed: bool,
    rounded: bool,
}
impl LoadingImage {
    pub fn new(id: impl Into<SharedString>, path: Option<PathBuf>) -> Self {
        let id = id.into();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hash);
        Self {
            id: id.into(),
            seed: hash.finish(),
            path,
            style: StyleRefinement::default(),
            rotation: None,
            maximum: None,
            failed: false,
            rounded: true,
        }
    }
    pub fn fit_within(mut self, maximum: Size<Pixels>) -> Self {
        self.maximum = Some(maximum);
        self
    }
    pub fn failed(mut self, failed: bool) -> Self {
        self.failed = failed;
        self
    }
    pub fn square(mut self) -> Self {
        self.rounded = false;
        self
    }
    pub fn rotation(mut self, angle: f32, width: f32, height: f32) -> Self {
        self.rotation = Some((angle, width, height));
        self
    }
}
impl Styled for LoadingImage {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
impl IntoElement for LoadingImage {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for LoadingImage {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let now = cx.background_executor().now();
        let state = window.with_element_state(
            id.expect("image identity"),
            |retained: Option<Entity<LoadingState>>, _| {
                let state = retained.unwrap_or_else(|| {
                    cx.new(|cx| {
                        cx.on_release(|state: &mut LoadingState, cx| {
                            state.clear_blur(None, cx);
                            state.clear_rotation(cx);
                        })
                        .detach();
                        LoadingState::new(self.path.clone(), now)
                    })
                });
                (state.clone(), state)
            },
        );
        let reduce = cx.reduce_motion();
        let loaded = self.path.as_ref().and_then(|path| {
            window.use_asset::<ImgResourceLoader>(&Resource::from(path.clone()), cx)
        });
        let failed = self.failed || loaded.as_ref().is_some_and(Result::is_err);
        let image = loaded.and_then(Result::ok);
        let (time, reveal, blurs) = state.update(cx, |state, cx| {
            if state.path != self.path {
                state.clear_rotation(cx);
                // Receiving the downloaded path continues the existing placeholder.
                if state.path.is_some() {
                    state.clear_blur(Some(window), cx);
                    state.ready = None;
                    state.painted_loading = false;
                    state.started = now;
                }
                state.path = self.path.clone();
            }
            if let Some(image) = &image {
                if reduce || !state.painted_loading {
                    state.ready = Some(now - REVEAL_DURATION);
                } else if state.ready.is_none() {
                    let source = BlurSource(image.clone());
                    state.blur_source = Some(source.clone());
                    if let Some(frames) = window.use_asset::<BlurAsset>(&source, cx) {
                        state.blur = Some(frames);
                        state.ready = Some(now);
                    }
                }
            }
            let reveal = progress(state.ready, now);
            if reveal >= 1. {
                state.clear_blur(Some(window), cx);
            }
            (
                now.saturating_duration_since(state.started).as_secs_f32(),
                reveal,
                state.blur.clone(),
            )
        });
        let owner = window.current_view();
        let rotated = if let (Some(image), Some((angle, width, height))) = (
            &image,
            self.rotation.filter(|(angle, _, _)| angle.abs() > 0.001),
        ) {
            state.update(cx, |state, cx| {
                if state.rotation_task.is_none()
                    && (!state.rotated_angle.is_finite()
                        || (angle - state.rotated_angle).abs() > 0.001)
                {
                    let prepared = state.prepared.clone();
                    let source = image.clone();
                    let task = cx.background_executor().spawn(async move {
                        let prepared = prepared.unwrap_or_else(|| {
                            let size = source.size(0);
                            let pixels = image::RgbaImage::from_raw(
                                size.width.0.max(1) as u32,
                                size.height.0.max(1) as u32,
                                source.as_bytes(0).unwrap_or_default().to_vec(),
                            )
                            .unwrap_or_else(|| image::RgbaImage::new(1, 1));
                            Arc::new(image::imageops::resize(
                                &pixels,
                                (width * 2.).round().max(1.) as u32,
                                (height * 2.).round().max(1.) as u32,
                                image::imageops::FilterType::Triangle,
                            ))
                        });
                        let frame = Arc::new(RenderImage::new(vec![image::Frame::new(
                            crate::card_preview::tilt(&prepared, angle),
                        )]));
                        (prepared, frame)
                    });
                    state.rotation_task = Some(cx.spawn(async move |state, cx| {
                        let (prepared, frame) = task.await;
                        let _ = state.update(cx, |state, cx| {
                            if let Some(previous) = state.rotated.replace(frame) {
                                cx.drop_image(previous, None);
                            }
                            state.prepared = Some(prepared);
                            state.rotated_angle = angle;
                            state.rotation_task = None;
                            App::notify(cx, owner);
                        });
                    }));
                }
                state.rotated.clone()
            })
        } else {
            state.update(cx, |state, cx| state.clear_rotation(cx));
            None
        };
        let phase = if failed {
            "image-error"
        } else if reveal >= 1. {
            "image-ready"
        } else if image.is_some() && reveal > 0. {
            "image-revealing"
        } else {
            "image-loading"
        };
        let mut root = div()
            .debug_selector(move || phase.into())
            .relative()
            .when(self.rounded, |view| view.rounded_lg());
        if let Some(maximum) = self.maximum {
            let dimensions = image.as_ref().map(|image| image.size(0));
            let (width, height) = dimensions.map_or((192., 96.), |s| {
                (s.width.0.max(1) as f32, s.height.0.max(1) as f32)
            });
            let factor = (f32::from(maximum.width) / width)
                .min(f32::from(maximum.height) / height)
                .min(1.);
            root = root.w(px(width * factor)).h(px(height * factor));
        }
        root.style().refine(&self.style);
        let seed = self.seed;
        let dark = cx.theme().mode.is_dark();
        if reveal < 1. && !failed {
            let fade = 1. - smooth(reveal);
            let mark = state.clone();
            let rounded = self.rounded;
            let phase = if reduce {
                0
            } else {
                (time * PLACEHOLDER_FPS).floor() as u64
            };
            let sample = (seed, dark, phase);
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .when(rounded, |view| view.rounded_lg())
                    .overflow_hidden()
                    .opacity(fade)
                    .bg(hsla(
                        random(seed, 30),
                        0.24,
                        if dark { 0.16 } else { 0.82 },
                        1.,
                    ))
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                let visible = bounds.intersects(&window.content_mask().bounds);
                                if visible {
                                    mark.update(cx, |state, _| state.painted_loading = true);
                                }
                                visible
                            },
                            move |bounds, visible, window, cx| {
                                if !visible {
                                    return;
                                }
                                let active = window.is_window_active();
                                let cloud = state.update(cx, |state, cx| {
                                    if state.cloud_task.is_none()
                                        && state.cloud_sample != Some(sample)
                                        && (active || state.cloud.is_none())
                                    {
                                        let job = cx.background_executor().spawn(async move {
                                            Arc::new(RenderImage::new(vec![image::Frame::new(
                                                cloud_pixels(
                                                    seed,
                                                    dark,
                                                    phase as f32 / PLACEHOLDER_FPS,
                                                ),
                                            )]))
                                        });
                                        state.cloud_task =
                                            Some(cx.spawn(async move |state, cx| {
                                                let image = job.await;
                                                let _ = state.update(cx, |state, cx| {
                                                    if let Some(previous) =
                                                        state.cloud.replace(image)
                                                    {
                                                        cx.drop_image(previous, None);
                                                    }
                                                    state.cloud_sample = Some(sample);
                                                    state.cloud_task = None;
                                                    App::notify(cx, owner);
                                                });
                                            }));
                                    }
                                    state.cloud.clone()
                                });
                                if let Some(cloud) = cloud {
                                    let radius = if rounded { px(8.) } else { px(0.) };
                                    if let Err(error) = window.paint_image(
                                        bounds,
                                        bounds,
                                        Corners::all(radius),
                                        cloud,
                                        0,
                                        false,
                                    ) {
                                        tracing::debug!(%error,"paint loading placeholder failed");
                                    }
                                }
                                if active && !reduce {
                                    window.request_animation_frame();
                                }
                            },
                        )
                        .size_full(),
                    ),
            );
        }
        if let Some(image) = image {
            let content = {
                img(rotated.unwrap_or(image))
                    .id("animated-image")
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .when(self.rounded, |image| image.rounded_lg())
                    .into_any_element()
            };
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .when(self.rounded, |view| view.rounded_lg())
                    .overflow_hidden()
                    .opacity(smooth(reveal))
                    .child(content),
            );
        }
        if let Some(frames) = blurs.filter(|frames| !frames.is_empty()) {
            let value = reveal * (frames.len() - 1) as f32;
            let index = (value.floor() as usize).min(frames.len() - 1);
            let blend = value - index as f32;
            for (frame, alpha) in [
                (index, 1. - blend),
                ((index + 1).min(frames.len() - 1), blend),
            ] {
                root = root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .when(self.rounded, |view| view.rounded_lg())
                        .overflow_hidden()
                        .opacity(alpha * (1. - reveal) * smooth((reveal * 4.).min(1.)))
                        .child(
                            img(frames[frame].clone())
                                .when(self.rounded, |image| image.rounded_lg())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        ),
                );
            }
        }
        if failed {
            root = root.child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .child(rust_i18n::t!("common.load_failed")),
            );
        }
        let mut element = root.into_any_element();
        let layout = element.request_layout(window, cx);
        (layout, element)
    }
    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{REVEAL_DURATION, cloud_pixels, progress};
    use std::time::{Duration, Instant};
    #[test]
    fn blurred_placeholder_is_opaque_soft_seeded_and_continuously_changing() {
        for dark in [false, true] {
            let first = cloud_pixels(42, dark, 0.);
            let later = cloud_pixels(42, dark, 3.);
            let nearby = cloud_pixels(42, dark, 1. / 24.);
            assert!(first.pixels().all(|pixel| pixel[3] == 255));
            assert_ne!(first, cloud_pixels(43, dark, 0.));
            assert_ne!(first, later);
            let difference = |a: &image::RgbaImage, b: &image::RgbaImage| {
                a.as_raw()
                    .iter()
                    .zip(b.as_raw())
                    .map(|(a, b)| a.abs_diff(*b) as usize)
                    .sum::<usize>()
            };
            assert!(difference(&first, &nearby) < difference(&first, &later));
            for y in 0..48 {
                for x in 1..48 {
                    for channel in 0..3 {
                        assert!(
                            first.get_pixel(x, y)[channel]
                                .abs_diff(first.get_pixel(x - 1, y)[channel])
                                <= 4,
                            "placeholder must have blurred contours"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn reveal_has_a_bounded_tween_and_finishes() {
        let now = Instant::now();
        assert_eq!(progress(None, now), 0.);
        assert_eq!(progress(Some(now), now), 0.);
        assert_eq!(progress(Some(now), now + REVEAL_DURATION), 1.);
        assert_eq!(progress(Some(now), now + Duration::from_secs(2)), 1.);
        assert!(progress(Some(now), now + Duration::from_millis(240)) > 0.4);
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod ui_tests {
    use super::{LoadingImage, blur_frames};
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit::{
        AnyWindowHandle, AppContext, Context, IntoElement, ParentElement, Render, RenderImage,
        Styled, Window, WindowOptions, div, px,
    };
    use std::{path::PathBuf, time::Duration};
    struct Harness {
        path: Option<PathBuf>,
        show: bool,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut root = div().size_full().p(px(24.));
            if self.show {
                root = root.child(
                    LoadingImage::new("test-image", self.path.clone())
                        .w(px(160.))
                        .h(px(120.)),
                );
            }
            root
        }
    }
    fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.run_until_parked();
    }
    #[gpui::test]
    fn loading_reveals_then_settles_and_cached_images_do_not_replay(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        image::RgbaImage::from_fn(160, 120, |x, y| image::Rgba([x as u8, y as u8, 100, 255]))
            .save(&path)
            .unwrap();
        let (window, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| Harness {
                    path: None,
                    show: true,
                })
            })
            .unwrap()
        });
        draw(cx, window);
        assert!(
            VisualTestContext::from_window(window, cx)
                .debug_bounds("image-loading")
                .is_some()
        );
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.path = Some(path);
                cx.notify();
            })
        });
        for _ in 0..5 {
            draw(cx, window);
        }
        cx.background_executor
            .advance_clock(Duration::from_millis(100));
        draw(cx, window);
        assert!(
            VisualTestContext::from_window(window, cx)
                .debug_bounds("image-revealing")
                .is_some()
        );
        cx.background_executor
            .advance_clock(Duration::from_millis(500));
        draw(cx, window);
        assert!(
            VisualTestContext::from_window(window, cx)
                .debug_bounds("image-ready")
                .is_some()
        );
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show = false;
                cx.notify();
            })
        });
        draw(cx, window);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show = true;
                cx.notify();
            })
        });
        draw(cx, window);
        assert!(
            VisualTestContext::from_window(window, cx)
                .debug_bounds("image-ready")
                .is_some()
        );
    }
    #[test]
    fn blur_frames_are_small_and_leave_source_bytes_unchanged() {
        let source = RenderImage::new(vec![image::Frame::new(image::RgbaImage::from_fn(
            240,
            120,
            |x, y| image::Rgba([(x % 255) as u8, (y % 255) as u8, 80, 255]),
        ))]);
        let bytes = source.as_bytes(0).unwrap().to_vec();
        let blurred = blur_frames(&source);
        assert_eq!(blurred.len(), 4);
        assert!(
            blurred
                .iter()
                .all(|frame| frame.size(0).width.0 <= 96 && frame.size(0).height.0 <= 96)
        );
        assert_ne!(blurred[0].as_bytes(0), blurred[3].as_bytes(0));
        assert_eq!(source.as_bytes(0).unwrap(), bytes);
    }
}
