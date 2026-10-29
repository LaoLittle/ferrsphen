use alhazen_core::{Player, PlayerConfig, PlayerEvent, PlayerState, Source, VideoFrame};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable,
        button::{Button, ButtonVariants},
        slider::{Slider, SliderEvent, SliderState},
        spinner::Spinner,
    },
    prelude::FluentBuilder,
    *,
};
use rust_i18n::t;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub fn extract_preview(source: &std::path::Path, output: &std::path::Path) -> anyhow::Result<()> {
    let player = Player::open(
        Source::File(source.to_owned()),
        PlayerConfig {
            audio_output: alhazen_core::audio::AudioOutputConfig::Disabled,
            max_output_size: Some((768, 704)),
            ..Default::default()
        },
    )?;
    let started = Instant::now();
    loop {
        if let Some(VideoFrame::Cpu {
            width,
            height,
            bgra,
            ..
        }) = player.current_frame()
        {
            let mut rgba = bgra.to_vec();
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            let image = image::RgbaImage::from_raw(width, height, rgba)
                .ok_or_else(|| anyhow::anyhow!("Invalid video frame dimensions"))?;
            let temporary =
                output.with_file_name(format!(".{:016x}.png.part", rand::random::<u64>()));
            image.save_with_format(&temporary, image::ImageFormat::Png)?;
            if let Err(error) = std::fs::rename(&temporary, output) {
                let _ = std::fs::remove_file(&temporary);
                if !output.is_file() {
                    return Err(error.into());
                }
            }
            return Ok(());
        }
        if let PlayerState::Error(error) = player.state() {
            anyhow::bail!("{error:?}");
        }
        anyhow::ensure!(
            started.elapsed() < Duration::from_secs(10),
            "Video preview timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub struct CloseRequested;

impl EventEmitter<CloseRequested> for VideoView {}

pub struct VideoView {
    embedded: bool,
    player: Option<Player>,
    frame: Option<(Arc<RenderImage>, Arc<[u8]>)>,
    retired: VecDeque<(Instant, Arc<RenderImage>)>,
    path: Option<PathBuf>,
    opening: bool,
    error: Option<String>,
    progress: Entity<SliderState>,
    volume: Entity<SliderState>,
    scrubbing: bool,
    muted: bool,
    focus: FocusHandle,
    tasks: Vec<Task<()>>,
    subscriptions: Vec<Subscription>,
}
impl VideoView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let progress = cx.new(|_| SliderState::new().min(0.).max(1.).step(0.001));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(1.)
        });
        let subscriptions = vec![
            cx.subscribe_in(&progress, window, |this, _, event, _, cx| {
                match event {
                    SliderEvent::Change(_) => this.scrubbing = true,
                    SliderEvent::Release(value) => {
                        this.scrubbing = false;
                        if let Some(player) = &this.player {
                            let duration = player.duration().unwrap_or_default();
                            player.seek(duration.mul_f32(value.start().clamp(0., 1.)));
                        }
                    }
                }
                cx.notify();
            }),
            cx.subscribe(&volume, |this, _, event, cx| {
                if let SliderEvent::Change(value) = event
                    && let Some(player) = &this.player
                {
                    player.set_volume(value.start());
                }
                cx.notify();
            }),
        ];
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        cx.on_release(|this, cx| {
            if let Some((image, _)) = this.frame.take() {
                cx.drop_image(image, None);
            }
            for (_, image) in this.retired.drain(..) {
                cx.drop_image(image, None);
            }
            if let Some(player) = this.player.take() {
                cx.background_executor()
                    .spawn(async move {
                        drop(player);
                    })
                    .detach();
            }
        })
        .detach();
        Self {
            embedded: false,
            player: None,
            frame: None,
            retired: VecDeque::new(),
            path: None,
            opening: false,
            error: None,
            progress,
            volume,
            scrubbing: false,
            muted: false,
            focus,
            tasks: Vec::new(),
            subscriptions,
        }
    }
    pub fn watch(&mut self, subscription: Subscription) {
        self.subscriptions.push(subscription);
    }
    pub fn embedded(mut self) -> Self {
        self.embedded = true;
        self
    }
    pub fn complete_download(&mut self, result: Result<PathBuf, String>, cx: &mut Context<Self>) {
        if self.opening || self.path.is_some() || self.error.is_some() {
            return;
        }
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.path = Some(path.clone());
        self.opening = true;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Player::open(
                        Source::File(path),
                        PlayerConfig {
                            autoplay: true,
                            max_output_size: Some((1280, 720)),
                            ..Default::default()
                        },
                    )
                })
                .await;
            let events = this
                .update(cx, |this, cx| {
                    this.opening = false;
                    match result {
                        Ok(player) => {
                            let events = player.events();
                            player.set_volume(this.volume.read(cx).value().start());
                            player.set_muted(this.muted);
                            this.player = Some(player);
                            cx.notify();
                            Some(events)
                        }
                        Err(error) => {
                            tracing::error!(?error, "open video failed");
                            this.error = Some(error.to_string());
                            cx.notify();
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(events) = events else {
                return;
            };
            let mut interval = Duration::from_millis(16);
            loop {
                cx.background_executor().timer(interval).await;
                let events = events.try_iter().collect::<Vec<_>>();
                let active = this.update(cx, |this, cx| {
                    for event in events {
                        match event {
                            PlayerEvent::Error(error) => {
                                tracing::error!(?error, "video playback failed");
                                this.error = Some(error.to_string());
                            }
                            PlayerEvent::Warning(warning) => {
                                tracing::warn!(%warning, "video playback warning")
                            }
                            _ => {}
                        }
                    }
                    this.refresh_frame(cx);
                    cx.notify();
                    this.player.as_ref().is_some_and(|player| {
                        matches!(
                            player.state(),
                            PlayerState::Playing | PlayerState::Buffering
                        )
                    })
                });
                match active {
                    Ok(true) => interval = Duration::from_millis(16),
                    Ok(false) => interval = Duration::from_millis(100),
                    Err(_) => break,
                }
            }
        });
        self.tasks.push(task);
    }
    fn refresh_frame(&mut self, cx: &mut Context<Self>) {
        if let Some(VideoFrame::Cpu {
            width,
            height,
            bgra,
            ..
        }) = self.player.as_ref().and_then(Player::current_frame)
            && !self
                .frame
                .as_ref()
                .is_some_and(|(_, previous)| Arc::ptr_eq(previous, &bgra))
            && let Some(pixels) = image::RgbaImage::from_raw(width, height, bgra.to_vec())
        {
            let image = Arc::new(RenderImage::new([image::Frame::new(pixels)]));
            if let Some((previous, _)) = self.frame.replace((image, bgra)) {
                self.retired.push_back((Instant::now(), previous));
            }
        }
        while self
            .retired
            .front()
            .is_some_and(|(at, _)| at.elapsed() >= Duration::from_millis(250))
        {
            if let Some((_, image)) = self.retired.pop_front() {
                cx.drop_image(image, None);
            }
        }
    }
    fn toggle(&self) {
        if let Some(player) = &self.player {
            match player.state() {
                PlayerState::Playing | PlayerState::Buffering => player.pause(),
                PlayerState::Ended => {
                    player.seek(Duration::ZERO);
                    player.play();
                }
                _ => player.play(),
            }
        }
    }
    fn seek_by(&self, seconds: f64) {
        if let Some(player) = &self.player {
            let target = (player.position().as_secs_f64() + seconds).max(0.);
            let target = target.min(
                player
                    .duration()
                    .map_or(target, |duration| duration.as_secs_f64()),
            );
            player.seek(Duration::from_secs_f64(target));
        }
    }
}
impl Render for VideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let playing = self.player.as_ref().is_some_and(|player| {
            matches!(
                player.state(),
                PlayerState::Playing | PlayerState::Buffering
            )
        });
        let position = self
            .player
            .as_ref()
            .map_or(Duration::ZERO, Player::position);
        let duration = self
            .player
            .as_ref()
            .and_then(Player::duration)
            .unwrap_or_default();
        if !self.scrubbing {
            let fraction = if duration.is_zero() {
                0.
            } else {
                (position.as_secs_f64() / duration.as_secs_f64()).clamp(0., 1.) as f32
            };
            if (self.progress.read(cx).value().start() - fraction).abs() > 0.001 {
                self.progress
                    .update(cx, |state, cx| state.set_value(fraction, window, cx));
            }
        }
        let content = div()
            .id("video-player")
            .debug_selector(|| "video-player".into())
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x151515))
            .text_color(cx.theme().foreground)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "space" => this.toggle(),
                    "left" => this.seek_by(-5.),
                    "right" => this.seek_by(5.),
                    "f" => window.toggle_fullscreen(),
                    "escape" if this.embedded => cx.emit(CloseRequested),
                    "escape" if window.is_fullscreen() => window.toggle_fullscreen(),
                    "escape" => {
                        let handle = window.window_handle();
                        cx.defer(move |cx| {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                        });
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .when(!self.embedded, |view| {
                view.child(
                    crate::window_chrome::title_bar(cx)
                        .h_12()
                        .child(div().flex_1()),
                )
            })
            .child(
                div()
                    .flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            if event.click_count == 2 {
                                window.toggle_fullscreen();
                            } else {
                                this.toggle();
                            }
                            cx.notify();
                        }),
                    )
                    .when_some(self.frame.as_ref(), |view, (frame, _)| {
                        view.child(
                            img(frame.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    })
                    .when(
                        self.error.is_none() && (self.opening || self.player.is_none()),
                        |view| view.child(Spinner::new()),
                    )
                    .when_some(self.error.clone(), |view, error| {
                        view.child(
                            div()
                                .debug_selector(|| "video-error".into())
                                .p_4()
                                .text_sm()
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .p_4()
                    .gap_3()
                    .child(
                        Slider::new(&self.progress)
                            .disabled(duration.is_zero())
                            .accessibility_label(t!("video.progress")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                Button::new("video-play")
                                    .ghost()
                                    .icon(if playing {
                                        IconName::Pause
                                    } else {
                                        IconName::Play
                                    })
                                    .accessibility_label(if playing {
                                        t!("video.pause")
                                    } else {
                                        t!("video.play")
                                    })
                                    .disabled(self.player.is_none())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle();
                                        cx.notify();
                                    })),
                            )
                            .child(div().text_xs().child(format!(
                                "{} / {}",
                                timestamp(position),
                                timestamp(duration)
                            )))
                            .child(div().flex_1())
                            .child(
                                Button::new("video-mute")
                                    .ghost()
                                    .icon(if self.muted {
                                        IconName::VolumeX
                                    } else {
                                        IconName::Volume2
                                    })
                                    .accessibility_label(t!("video.mute"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.muted = !this.muted;
                                        if let Some(player) = &this.player {
                                            player.set_muted(this.muted);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(div().w_24().child(
                                Slider::new(&self.volume).accessibility_label(t!("video.volume")),
                            ))
                            .when_some(self.path.clone(), |bar, path| {
                                bar.child(
                                    Button::new("video-reveal")
                                        .ghost()
                                        .icon(IconName::FolderOpen)
                                        .tooltip(t!("common.local_file"))
                                        .accessibility_label(t!("common.local_file"))
                                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                                )
                            })
                            .child(
                                Button::new("video-fullscreen")
                                    .ghost()
                                    .icon(IconName::Maximize)
                                    .accessibility_label(t!("video.fullscreen"))
                                    .on_click(|_, window, _| window.toggle_fullscreen()),
                            ),
                    ),
            );
        if self.embedded {
            content.into_any_element()
        } else {
            crate::window_chrome::surface(content, window, cx).into_any_element()
        }
    }
}
fn timestamp(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use alhazen_core::{
        Player, PlayerConfig, PlayerState, Source, VideoFrame, audio::AudioOutputConfig,
    };
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn h264_aac_decodes_pauses_seeks_and_ends_without_a_window() {
        if !Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("ffmpeg unavailable; skipping H.264/AAC playback test");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.mp4");
        let output = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=160x90:rate=20:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-y",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let preview = directory.path().join("preview.png");
        super::extract_preview(&path, &preview).unwrap();
        let image = image::open(preview).unwrap().to_rgb8();
        assert_eq!(image.dimensions(), (160, 90));
        assert!(image.pixels().any(|pixel| pixel[0].abs_diff(pixel[2]) > 60));
        let player = Player::open(
            Source::File(path),
            PlayerConfig {
                audio_output: AudioOutputConfig::Disabled,
                prefer_hardware: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(player.has_video());
        assert!(player.duration().unwrap() >= Duration::from_secs(1));
        player.play();
        wait_for(&player, "play", || {
            player.current_frame().is_some() && player.position() >= Duration::from_millis(150)
        });
        player.pause();
        let paused = player.position();
        std::thread::sleep(Duration::from_millis(80));
        assert!(player.position().abs_diff(paused) < Duration::from_millis(20));
        player.seek(Duration::from_millis(1100));
        wait_for(&player, "seek", || {
            player.current_frame().is_some_and(|frame| {
                let VideoFrame::Cpu {
                    pts,
                    width,
                    height,
                    bgra,
                } = frame;
                assert_eq!((width, height), (160, 90));
                assert_eq!(bgra.len(), (width * height * 4) as usize);
                pts >= Duration::from_secs(1)
            })
        });
        player.play();
        wait_for(&player, "end", || {
            player.current_frame();
            matches!(player.state(), PlayerState::Ended)
        });
    }
    fn wait_for(player: &Player, stage: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "{stage} timed out: {:?}, position {:?}, events {:?}",
                player.state(),
                player.position(),
                player.events().try_iter().collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
