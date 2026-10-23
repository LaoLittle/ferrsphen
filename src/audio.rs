use alhazen_core::{Player, PlayerConfig, PlayerEvent, PlayerState, Source};
use gpui_kit::*;
use std::{collections::HashMap, path::PathBuf, time::Duration};

#[derive(Default)]
pub struct VoicePlayer {
    pub key: Option<String>,
    pub loading: bool,
    pub playing: bool,
    pub position: Duration,
    pub duration: Duration,
    pub error: Option<String>,
    cache: HashMap<String, PathBuf>,
    player: Option<Player>,
    task: Option<Task<()>>,
    generation: u64,
}
impl VoicePlayer {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            cx.on_release(|this: &mut Self, cx| this.release_player(cx))
                .detach();
            Self::default()
        })
    }
    fn release_player(&mut self, cx: &mut App) {
        if let Some(player) = self.player.take() {
            player.pause();
            cx.background_executor()
                .spawn(async move {
                    drop(player);
                })
                .detach();
        }
    }
    /// Returns whether the selected message needs to be downloaded.
    pub fn toggle(&mut self, key: String, cx: &mut Context<Self>) -> bool {
        if self.key.as_ref() == Some(&key) && self.error.is_none() {
            if let Some(player) = &self.player {
                if self.playing {
                    player.pause();
                    self.playing = false;
                } else {
                    if matches!(player.state(), PlayerState::Ended) {
                        player.seek(Duration::ZERO);
                    }
                    player.play();
                    self.playing = true;
                }
            } else if self.loading {
                self.stop(cx);
            }
            cx.notify();
            return false;
        }
        self.stop(cx);
        self.key = Some(key.clone());
        self.loading = true;
        let cached = self.cache.get(&key).cloned();
        if let Some(path) = cached {
            self.complete(key, Ok(path), cx);
        }
        cx.notify();
        self.task.is_none()
    }
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.task = None;
        self.release_player(cx);
        self.key = None;
        self.loading = false;
        self.playing = false;
        self.position = Duration::ZERO;
        self.duration = Duration::ZERO;
        self.error = None;
        cx.notify();
    }
    pub fn complete(
        &mut self,
        key: String,
        result: Result<PathBuf, String>,
        cx: &mut Context<Self>,
    ) {
        if let Ok(path) = &result {
            self.cache.insert(key.clone(), path.clone());
        }
        if self.key.as_ref() != Some(&key) || self.task.is_some() {
            return;
        }
        let path = match result {
            Ok(path) => path,
            Err(error) => {
                self.loading = false;
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let generation = self.generation;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Player::open(
                        Source::File(path),
                        PlayerConfig {
                            autoplay: true,
                            ..Default::default()
                        },
                    )
                })
                .await;
            let events = this
                .update(cx, |this, cx| {
                    if generation != this.generation {
                        return None;
                    }
                    this.loading = false;
                    match result {
                        Ok(player) => {
                            this.duration = player.duration().unwrap_or_default();
                            this.playing = true;
                            let events = player.events();
                            this.player = Some(player);
                            cx.notify();
                            Some(events)
                        }
                        Err(error) => {
                            tracing::error!(?error, "open voice failed");
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
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if generation != this.generation {
                            return false;
                        }
                        let previous = (
                            this.loading,
                            this.playing,
                            this.position,
                            this.error.clone(),
                        );
                        for event in events.try_iter() {
                            match event {
                                PlayerEvent::Warning(warning) => {
                                    tracing::warn!(%warning, "voice playback warning")
                                }
                                PlayerEvent::Error(error) => {
                                    tracing::error!(?error, "voice playback failed");
                                    this.error = Some(error.to_string());
                                }
                                _ => {}
                            }
                        }
                        if let Some(player) = &this.player {
                            let _ = player.current_frame();
                            let position = player.position();
                            let state = player.state();
                            this.loading =
                                matches!(state, PlayerState::Loading | PlayerState::Buffering);
                            let playing =
                                matches!(state, PlayerState::Playing | PlayerState::Buffering);
                            this.position = position;
                            this.playing = playing && this.error.is_none();
                            if this.error.is_some() {
                                this.loading = false;
                            }
                        }
                        if previous
                            != (
                                this.loading,
                                this.playing,
                                this.position,
                                this.error.clone(),
                            )
                        {
                            cx.notify();
                        }
                        this.error.is_none()
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
            }
        }));
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::VoicePlayer;
    use gpui::TestAppContext;
    use std::path::PathBuf;

    #[gpui::test]
    fn switching_voice_ignores_late_download_and_allows_cancel_and_retry(cx: &mut TestAppContext) {
        let player = cx.update(VoicePlayer::new);
        cx.update(|cx| {
            player.update(cx, |player, cx| {
                assert!(player.toggle("first".into(), cx));
                assert!(player.loading);
                assert!(player.toggle("second".into(), cx));
                player.complete("first".into(), Err("late failure".into()), cx);
                assert_eq!(player.key.as_deref(), Some("second"));
                assert!(player.error.is_none());
                player.complete("second".into(), Err("download failed".into()), cx);
                assert!(!player.loading);
                assert!(player.error.is_some());
                assert!(player.toggle("second".into(), cx));
                assert!(player.error.is_none());
                assert!(!player.toggle("second".into(), cx));
                assert!(player.key.is_none());
                player.complete("second".into(), Ok("/cached.wav".into()), cx);
                assert!(player.key.is_none());
                assert!(player.player.is_none());
                assert_eq!(player.cache["second"], PathBuf::from("/cached.wav"));
            })
        });
    }
}
