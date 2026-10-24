use atri_qq::{
    contacts::GroupMember,
    messages::{Conversation, Mention, Message, MessageElement, PushEvent, Reply},
};
use ferrsphen::ui_motion::{CardTextureCache, card_frame};
use ferrsphen::{
    backend::{AttachmentKind, Command, ComposePart, Update},
    model::{Model, conversation_key, image_only, message_groups, reply_target, summary},
    ui_motion::{Bounce, ENTRANCE_SPAN, ENTRANCE_ZOOM, EXIT_SPAN, TimelineViewport, page_progress},
    viewer::{ImageItem, Viewer},
};
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        popover::Popover,
        spinner::Spinner,
        *,
    },
    prelude::FluentBuilder,
    *,
};
use rust_i18n::t;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const REACTION_CHOICES: [u32; 12] = [66, 76, 77, 13, 0, 32, 28, 5, 21, 49, 89, 79];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Chat,
    Settings,
}

struct PendingCopy {
    message: Message,
    baseline: Option<ClipboardItem>,
}
struct AttachmentConfirmation {
    conversation: Conversation,
    items: Vec<(PathBuf, AttachmentKind)>,
}

enum Phase {
    Connecting,
    Login,
    Chat,
}
struct Fade {
    from: f32,
    to: f32,
    started: Instant,
    generation: u64,
}
impl Fade {
    fn value(&self) -> f32 {
        let progress = (self.started.elapsed().as_secs_f32() / 0.08).min(1.);
        self.from + (self.to - self.from) * progress
    }
}
#[derive(Clone, Copy)]
enum DetailKind {
    Image,
    Forward,
}
struct ReplyJump {
    conversation: Conversation,
    reply: Reply,
    read_only: bool,
    waiting: bool,
    layout_frames: u8,
    motion: Option<(SpringState, Instant)>,
}
struct SendScroll {
    conversation: String,
    from: Pixels,
    target: Pixels,
    started: Instant,
}
struct RecallTween {
    original: gpui_kit::Size<Pixels>,
    started: Instant,
}
impl RecallTween {
    fn state(&self, now: Instant) -> SpringState {
        SpringConfig::new(280., 30., 1.).step(
            SpringState {
                position: 0.,
                velocity: 0.,
            },
            1.,
            now.saturating_duration_since(self.started).as_secs_f32(),
        )
    }
}
#[derive(Clone)]
struct TimelineAnchor {
    conversation: String,
    follow_tail: bool,
    message: String,
    top: Pixels,
    layout_frames: u8,
    settle_frames: u8,
}
struct ConversationSwitch {
    destination: Conversation,
    started: Instant,
    entering: bool,
}
struct DetailWindow {
    owner: Entity<AppView>,
    kind: DetailKind,
    _observer: Subscription,
}
impl Render for DetailWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self.owner.update(cx, |owner, cx| match self.kind {
            DetailKind::Image => {
                if owner.viewer.is_some() {
                    owner.image_viewer(window, cx)
                } else {
                    div().into_any_element()
                }
            }
            DetailKind::Forward => {
                owner.advance_reply_jump(true, window, cx);
                owner.forward_view(window, cx)
            }
        });
        ferrsphen::window_chrome::surface(content, window, cx)
    }
}
pub struct AppView {
    model: Model,
    viewer: Option<Viewer>,
    viewer_focus: FocusHandle,
    fades: HashMap<String, Fade>,
    expanded_images: HashSet<String>,
    card_textures: Entity<CardTextureCache>,
    bubble_sizes: Rc<RefCell<HashMap<String, gpui_kit::Size<Pixels>>>>,
    visible_bubbles: Rc<RefCell<HashMap<String, gpui_kit::Size<Pixels>>>>,
    recall_tweens: HashMap<String, RecallTween>,
    recall_animating: bool,
    recall_clock: BackgroundExecutor,
    viewer_window: Option<AnyWindowHandle>,
    video_windows: HashMap<String, AnyWindowHandle>,
    video_results: HashMap<String, Result<PathBuf, String>>,
    voice_player: Entity<ferrsphen::audio::VoicePlayer>,
    forward_window: Option<AnyWindowHandle>,
    forward_key: Option<String>,
    forward_error: Option<String>,
    forward_cache: HashMap<String, Vec<Message>>,
    phase: Phase,
    page: Page,
    mobile_platform: bool,
    desktop_windows: bool,
    window_kind: crate::window_chrome::AppWindowKind,
    window_switch_pending: bool,
    phone: bool,
    phone_conversation: bool,
    phone_qr_login: bool,
    commands: mpsc::Sender<Command>,
    account: Entity<InputState>,
    password: Entity<InputState>,
    search: Entity<InputState>,
    search_index: RefCell<ferrsphen::search::SearchIndex>,
    composer: Entity<TextareaState>,
    compose_images: HashMap<String, PathBuf>,
    compose_faces: HashMap<String, crate::expressions::Face>,
    emoji_open: bool,
    emoji_favorites: bool,
    favorites: Vec<crate::expressions::Favorite>,
    favorites_refresh: Option<Task<()>>,
    favorites_loading: bool,
    favorites_error: bool,
    poke_notices: HashMap<String, Vec<(Option<String>, String)>>,
    pending_pokes: Vec<crate::activity::ActivityRecord>,
    pending_reactions: HashSet<crate::activity::ReactionKey>,
    compose_cache: tempfile::TempDir,
    pending_copy: Option<PendingCopy>,
    draft_contents: HashMap<String, gpui_kit::component::input::InputContent>,
    confirmation: Option<AttachmentConfirmation>,
    confirmation_focus: FocusHandle,
    challenge: Option<atri_qq::LoginChallenge>,
    ticket: Entity<InputState>,
    captcha_random: Entity<InputState>,
    qr: Option<Arc<Image>>,
    qr_url: Option<String>,
    status: String,
    error: Option<String>,
    sidebar: bool,
    assets: HashMap<String, PathBuf>,
    requested: HashSet<String>,
    scroll: ScrollHandle,
    lazy_heights: Rc<RefCell<HashMap<String, Pixels>>>,
    bounce: Bounce,
    bounce_animating: bool,
    scroll_anchor: Option<TimelineAnchor>,
    loading_more: HashSet<String>,
    transition: Option<ConversationSwitch>,
    sequence: u64,
    reply: Option<Message>,
    forward: Option<Vec<Message>>,
    details: Option<String>,
    mentions: Vec<Mention>,
    retries: HashMap<u64, Command>,
    forward_scroll: ScrollHandle,
    reply_jump: Option<ReplyJump>,
    send_scroll: Option<SendScroll>,
    send_scroll_animating: bool,
    _subscriptions: Vec<Subscription>,
    _updates: Task<()>,
}
impl AppView {
    fn timeline_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        if delta.x.abs() > delta.y.abs() {
            return;
        }
        self.reply_jump = None;
        self.send_scroll = None;
        self.scroll_anchor = None;
        let before = self.scroll.offset();
        let previous_top = self.scroll.top_item();
        if cx.reduce_motion() {
            self.bounce = Bounce::default();
            let after = (before.y + delta.y).clamp(-self.scroll.max_offset().y.max(px(0.)), px(0.));
            self.scroll.set_offset(point(before.x, after));
        } else {
            let scroll = &self.scroll;
            self.bounce.packet(
                delta.y.into(),
                event.touch_phase,
                matches!(event.delta, ScrollDelta::Pixels(_)),
                scroll.bounds().size.height.into(),
                cx.background_executor().now(),
                |delta| {
                    let before = scroll.offset();
                    let after =
                        (before.y + px(delta)).clamp(-scroll.max_offset().y.max(px(0.)), px(0.));
                    scroll.set_offset(point(before.x, after));
                    (after - before.y).into()
                },
            );
        }
        let after = self.scroll.offset().y;
        if delta.y > px(0.) && after >= px(-1.) {
            self.load_history(true);
        }
        if self.scroll.top_item() != previous_top {
            self.request_assets();
        }
        self.animate_bounce(cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn animate_bounce(&mut self, cx: &mut Context<Self>) {
        if self.bounce_animating || self.bounce.position == 0. {
            return;
        }
        self.bounce_animating = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let active = this.update(cx, |this, cx| {
                    let active = if cx.reduce_motion() {
                        this.bounce = Bounce::default();
                        false
                    } else {
                        this.bounce.step(cx.background_executor().now())
                    };
                    this.bounce_animating = active;
                    cx.notify();
                    active
                });
                if !matches!(active, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn scroll_after_send(&mut self, conversation: &str, cx: &mut Context<Self>) {
        if self.model.selected.as_deref() != Some(conversation) {
            return;
        }
        self.reply_jump = None;
        self.scroll_anchor = None;
        self.bounce = Bounce::default();
        self.send_scroll = Some(SendScroll {
            conversation: conversation.into(),
            from: self.scroll.offset().y,
            target: -self.scroll.max_offset().y.max(px(0.)),
            started: cx.background_executor().now(),
        });
        if self.send_scroll_animating {
            return;
        }
        self.send_scroll_animating = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let active = this.update(cx, |this, cx| {
                    let active = this.advance_send_scroll(cx);
                    this.send_scroll_animating = active;
                    cx.notify();
                    active
                });
                if !matches!(active, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn advance_send_scroll(&mut self, cx: &Context<Self>) -> bool {
        let Some(mut motion) = self.send_scroll.take() else {
            return false;
        };
        if self.model.selected.as_ref() != Some(&motion.conversation)
            || !matches!(self.phase, Phase::Chat)
            || self.page != Page::Chat
        {
            return false;
        }
        let now = cx.background_executor().now();
        let target = -self.scroll.max_offset().y.max(px(0.));
        // Lazy rows and the composer can change the destination after sending.
        if (target - motion.target).abs() > px(0.5) {
            motion.from = self.scroll.offset().y;
            motion.target = target;
            motion.started = now;
        }
        let progress = if cx.reduce_motion() {
            1.
        } else {
            (now.saturating_duration_since(motion.started).as_secs_f32() / 0.28).min(1.)
        };
        let eased = 1. - (1. - progress).powi(3);
        self.scroll.set_offset(point(
            self.scroll.offset().x,
            motion.from + (motion.target - motion.from) * eased,
        ));
        self.request_assets();
        if progress < 1. || now.saturating_duration_since(motion.started).as_millis() < 380 {
            self.send_scroll = Some(motion);
            true
        } else {
            // Retain GPUI's tail anchor until the newly rendered content is laid out.
            self.scroll.scroll_to_bottom();
            false
        }
    }

    fn retry_send(&mut self, id: u64, key: &str, text: &str, cx: &mut Context<Self>) {
        if let Some(command) = self.retries.get(&id).cloned()
            && self.command(command)
        {
            self.model.pending.insert(id, (key.into(), text.into()));
            self.model.failures.remove(&id);
            self.scroll_after_send(key, cx);
        }
        cx.notify();
    }

    fn capture_timeline_anchor(&self) -> Option<TimelineAnchor> {
        let rows = self.model.messages.get(self.model.selected.as_ref()?)?;
        let index = self.scroll.top_item();
        let groups = message_groups(rows, &self.model.recalled);
        let message = groups.get(index)?.first()?;
        let bounds = self.scroll.bounds_for_item(index)?;
        Some(TimelineAnchor {
            conversation: self.model.selected.clone()?,
            follow_tail: false,
            message: message_key(message),
            top: bounds.top() + self.scroll.offset().y - self.scroll.bounds().top(),
            layout_frames: 1,
            settle_frames: 6,
        })
    }

    fn advance_timeline_anchor(&mut self, window: &mut Window) {
        let Some(mut anchor) = self.scroll_anchor.take() else {
            return;
        };
        if self.model.selected.as_ref() != Some(&anchor.conversation) {
            return;
        }
        if anchor.layout_frames > 0 {
            anchor.layout_frames -= 1;
            self.scroll_anchor = Some(anchor);
            window.request_animation_frame();
            return;
        }
        let target = self
            .model
            .selected
            .as_ref()
            .and_then(|key| self.model.messages.get(key))
            .and_then(|rows| {
                message_groups(rows, &self.model.recalled)
                    .into_iter()
                    .enumerate()
                    .find_map(|(index, group)| {
                        let position = group
                            .iter()
                            .position(|message| message_key(message) == anchor.message)?;
                        Some((
                            index,
                            if self.stack_expanded(group) {
                                image_message_offset(group, position)
                            } else {
                                0.
                            },
                        ))
                    })
            });
        if let Some((bounds, offset)) = target.and_then(|(index, offset)| {
            self.scroll
                .bounds_for_item(index)
                .map(|bounds| (bounds, offset))
        }) {
            let y = (self.scroll.bounds().top() + anchor.top - bounds.top() - px(offset))
                .clamp(-self.scroll.max_offset().y.max(px(0.)), px(0.));
            self.scroll.set_offset(point(self.scroll.offset().x, y));
        }
        if anchor.follow_tail {
            self.scroll.set_offset(point(
                self.scroll.offset().x,
                -self.scroll.max_offset().y.max(px(0.)),
            ));
        }
        if anchor.settle_frames > 0 {
            anchor.settle_frames -= 1;
            self.scroll_anchor = Some(anchor);
            window.request_animation_frame();
        }
        self.request_assets();
    }

    fn advance_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut transition) = self.transition.take() else {
            return;
        };
        let now = cx.background_executor().now();
        let span = if transition.entering {
            ENTRANCE_SPAN
        } else {
            EXIT_SPAN
        };
        if cx.reduce_motion() {
            if !transition.entering {
                self.commit_select(transition.destination, window, cx);
            }
            return;
        }
        if now.saturating_duration_since(transition.started) >= span {
            if transition.entering {
                return;
            }
            self.commit_select(transition.destination.clone(), window, cx);
            transition.entering = true;
            transition.started = now;
        }
        self.transition = Some(transition);
        window.request_animation_frame();
    }

    fn conversation_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let visible = self.transition.as_ref().map_or(1., |transition| {
            let span = if transition.entering {
                ENTRANCE_SPAN
            } else {
                EXIT_SPAN
            };
            let progress = (cx
                .background_executor()
                .now()
                .saturating_duration_since(transition.started)
                .as_secs_f32()
                / span.as_secs_f32())
            .clamp(0., 1.);
            if transition.entering {
                page_progress(progress)
            } else {
                1. - page_progress(progress)
            }
        });
        let scale = 1. - ENTRANCE_ZOOM * (1. - visible);
        div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .child(
                div()
                    .debug_selector(|| "conversation-panel".into())
                    .relative()
                    .flex()
                    .w(relative(scale))
                    .h(relative(scale))
                    .opacity(visible)
                    .child(self.conversation(window, cx))
                    .when(!self.phone && self.sidebar && self.is_group(), |panel| {
                        panel.child(self.info_sidebar(cx))
                    })
                    .when(self.transition.is_some(), |panel| {
                        panel.child(
                            div()
                                .absolute()
                                .inset_0()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn quoted_message(&self, message: &Message, reply: &Reply, read_only: bool) -> Message {
        if read_only
            && let Some(target) = self
                .forward
                .as_deref()
                .and_then(|rows| reply_target(rows, reply))
        {
            return target.clone();
        }
        self.model.reply_preview(&message.conversation, reply)
    }

    fn reply_card(&self, message: &Message, cx: &mut Context<Self>) -> AnyElement {
        let recalled = self.model.is_recalled(message);
        let images = if recalled {
            Vec::new()
        } else {
            message
                .elements
                .iter()
                .enumerate()
                .filter_map(|(index, element)| match element {
                    MessageElement::Image(_) => {
                        Some((index, self.assets.get(&image_key(message, index)).cloned()))
                    }
                    _ => None,
                })
                .take(3)
                .collect::<Vec<_>>()
        };
        let text = if recalled {
            t!("common.recalled").to_string()
        } else {
            let text = summary(
                &message
                    .elements
                    .iter()
                    .filter(|element| !matches!(element, MessageElement::Reply(_)))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            if text.is_empty() {
                t!("common.message").into_owned()
            } else {
                text
            }
        };
        div()
            .flex()
            .gap_2()
            .items_center()
            .min_w_0()
            .border_l_2()
            .border_color(cx.theme().primary)
            .pl_2()
            .py_1()
            .children(images.into_iter().map(|(index, path)| {
                div()
                    .size_12()
                    .flex_shrink_0()
                    .rounded_md()
                    .overflow_hidden()
                    .bg(cx.theme().muted)
                    .child(match path {
                        Some(path) => img(path)
                            .id(SharedString::from(format!(
                                "quote-image:{}:{index}",
                                message_key(message)
                            )))
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .into_any_element(),
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .child(t!("common.image"))
                            .into_any_element(),
                    })
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.model.sender_name(message)),
                    )
                    .child(div().truncate().max_w(rems(24.)).child(text)),
            )
            .into_any_element()
    }

    fn jump_to_reply(
        &mut self,
        conversation: Conversation,
        reply: Reply,
        read_only: bool,
        cx: &mut Context<Self>,
    ) {
        let rows = if read_only {
            self.forward.as_deref()
        } else {
            self.model
                .messages
                .get(&conversation_key(&conversation))
                .map(Vec::as_slice)
        };
        let found = rows.and_then(|rows| reply_target(rows, &reply)).is_some();
        if !found && read_only {
            self.forward_error = Some(t!("error.original_missing").into_owned());
            cx.notify();
            return;
        }
        if !found
            && !self.command(Command::LocateReply {
                conversation: conversation.clone(),
                reply: reply.clone(),
            })
        {
            return;
        }
        if read_only {
            self.forward_error = None;
        } else {
            self.send_scroll = None;
        }
        self.reply_jump = Some(ReplyJump {
            conversation,
            reply,
            read_only,
            waiting: !found,
            layout_frames: 1,
            motion: None,
        });
        self.request_assets();
        cx.notify();
    }

    fn advance_reply_jump(&mut self, read_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(jump) = self
            .reply_jump
            .as_ref()
            .filter(|jump| jump.read_only == read_only && !jump.waiting)
        else {
            return;
        };
        let rows = if read_only {
            self.forward.as_deref()
        } else {
            self.model
                .messages
                .get(&conversation_key(&jump.conversation))
                .map(Vec::as_slice)
        };
        let target = rows.and_then(|rows| {
            let message = reply_target(rows, &jump.reply)?;
            message_groups(rows, &self.model.recalled)
                .into_iter()
                .enumerate()
                .find(|(_, group)| group.iter().any(|row| std::ptr::eq(row, message)))
                .map(|(index, group)| {
                    let stacked =
                        image_only(&group[0]) && (group.len() > 1 || group[0].elements.len() > 1);
                    let offset = if stacked {
                        image_message_offset(
                            group,
                            group
                                .iter()
                                .position(|row| std::ptr::eq(row, message))
                                .unwrap_or_default(),
                        )
                    } else {
                        0.
                    };
                    (index, message_key(&group[0]), stacked, offset)
                })
        });
        let Some((index, key, stacked, image_offset)) = target else {
            if read_only {
                self.forward_error = Some(t!("error.original_missing").into_owned());
            } else {
                self.error = Some(t!("error.original_missing").into_owned());
            }
            self.reply_jump = None;
            return;
        };
        if stacked && self.expanded_images.insert(key) {
            self.reply_jump.as_mut().unwrap().layout_frames = 1;
        }
        let jump = self.reply_jump.as_mut().unwrap();
        if jump.layout_frames > 0 {
            jump.layout_frames -= 1;
            window.request_animation_frame();
            return;
        }
        let scroll = if read_only {
            &self.forward_scroll
        } else {
            &self.scroll
        };
        let Some(bounds) = scroll.bounds_for_item(index) else {
            window.request_animation_frame();
            return;
        };
        let viewport = scroll.bounds();
        let target = f32::from(
            viewport.top() + viewport.size.height * 0.3 - bounds.top() - px(image_offset),
        )
        .clamp(-f32::from(scroll.max_offset().y).max(0.), 0.);
        let now = cx.background_executor().now();
        let (state, last) = jump.motion.unwrap_or((
            SpringState {
                position: scroll.offset().y.into(),
                velocity: 0.,
            },
            now,
        ));
        let spring = SpringConfig::new(260., 32., 1.);
        let state = spring.step(
            state,
            target,
            now.saturating_duration_since(last).as_secs_f32(),
        );
        let settled = spring.is_settled(state, target, 0.5);
        scroll.set_offset(point(
            scroll.offset().x,
            px(if settled { target } else { state.position }),
        ));
        if settled {
            self.reply_jump = None;
        } else {
            jump.motion = Some((state, now));
            window.request_animation_frame();
        }
    }
    pub fn new(
        commands: mpsc::Sender<Command>,
        mut updates: mpsc::Receiver<Update>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mobile_platform = cfg!(any(target_os = "ios", target_os = "android"));
        let phone = mobile_platform && phone_layout(window.viewport_size());
        let account = cx.new(|cx| InputState::new(window, cx).placeholder(t!("login.account")));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("login.password"))
                .masked(true)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("chat.search")));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("chat.input"))
                .auto_grow(1, 6)
                .submit_on_enter(!phone)
        });
        let ticket = cx.new(|cx| InputState::new(window, cx).placeholder("ticket"));
        let captcha_random = cx.new(|cx| InputState::new(window, cx).placeholder("randstr"));
        let voice_player = ferrsphen::audio::VoicePlayer::new(cx);
        let task = cx.spawn(async move |this, cx| {
            while let Some(update) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match update {
                            Update::Voice { key, result } => this
                                .voice_player
                                .update(cx, |player, cx| player.complete(key, result, cx)),
                            update => {
                                if matches!(
                                    update,
                                    Update::Offline(_) | Update::Connecting | Update::LoginRequired
                                ) {
                                    this.voice_player.update(cx, |player, cx| player.stop(cx));
                                }
                                this.apply(update);
                                this.finish_copy(cx);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut view = Self {
            model: Model::default(),
            viewer: None,
            viewer_focus: cx.focus_handle(),
            fades: HashMap::new(),
            expanded_images: HashSet::new(),
            card_textures: CardTextureCache::new(cx),
            bubble_sizes: Rc::new(RefCell::new(HashMap::new())),
            visible_bubbles: Rc::new(RefCell::new(HashMap::new())),
            recall_tweens: HashMap::new(),
            recall_animating: false,
            recall_clock: cx.background_executor().clone(),
            viewer_window: None,
            video_windows: HashMap::new(),
            video_results: HashMap::new(),
            voice_player,
            forward_window: None,
            forward_key: None,
            forward_error: None,
            forward_cache: HashMap::new(),
            phase: Phase::Connecting,
            mobile_platform,
            desktop_windows: !mobile_platform,
            window_kind: crate::window_chrome::AppWindowKind::Login,
            window_switch_pending: false,
            page: Page::Chat,
            phone,
            phone_conversation: false,
            phone_qr_login: false,
            commands,
            account,
            password,
            search,
            search_index: RefCell::default(),
            composer,
            compose_images: HashMap::new(),
            compose_faces: HashMap::new(),
            emoji_open: false,
            emoji_favorites: false,
            favorites: Vec::new(),
            favorites_refresh: None,
            favorites_loading: false,
            favorites_error: false,
            poke_notices: HashMap::new(),
            pending_pokes: Vec::new(),
            pending_reactions: HashSet::new(),
            compose_cache: tempfile::tempdir().expect("draft cache"),
            pending_copy: None,
            draft_contents: HashMap::new(),
            confirmation: None,
            confirmation_focus: cx.focus_handle(),
            challenge: None,
            ticket,
            captcha_random,
            qr: None,
            qr_url: None,
            status: String::new(),
            error: None,
            sidebar: true,
            assets: HashMap::new(),
            requested: HashSet::new(),
            scroll: ScrollHandle::new(),
            lazy_heights: Rc::default(),
            bounce: Bounce::default(),
            bounce_animating: false,
            scroll_anchor: None,
            loading_more: HashSet::new(),
            transition: None,
            sequence: 0,
            reply: None,
            forward: None,
            details: None,
            mentions: Vec::new(),
            retries: HashMap::new(),
            forward_scroll: ScrollHandle::new(),
            reply_jump: None,
            send_scroll: None,
            send_scroll_animating: false,
            _subscriptions: Vec::new(),
            _updates: task,
        };
        view.bind_window(window, cx);
        view
    }
    fn bind_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        self._subscriptions = vec![
            cx.on_window_closed(move |cx, closed| {
                let owner = owner.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |this, cx| this.release_child_window(closed, cx));
                });
            }),
            cx.observe(&self.voice_player, |_, _, cx| cx.notify()),
            cx.subscribe_in(&self.search, window, |_, _, _: &InputEvent, _, cx| {
                cx.notify()
            }),
            cx.subscribe_in(&self.password, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.login(window, cx);
                }
            }),
            cx.subscribe_in(
                &self.composer,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } if !this.phone => {
                        this.send(window, cx)
                    }
                    InputEvent::Change => {
                        if let Some(key) = this.model.selected.clone() {
                            this.draft_contents.insert(key, input.read(cx).content());
                        }
                        if let Some(key) = this.model.selected.clone()
                            && let Some(chat) = this
                                .model
                                .chats
                                .iter_mut()
                                .find(|chat| conversation_key(&chat.conversation) == key)
                        {
                            chat.draft = input.read(cx).value().replace('\u{fffc}', "");
                        }
                        this.persist();
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];
    }
    fn release_child_window(&mut self, closed: WindowId, cx: &mut Context<Self>) {
        let mut changed = false;
        if self
            .viewer_window
            .is_some_and(|window| window.window_id() == closed)
        {
            self.viewer_window = None;
            self.viewer = None;
            changed = true;
        }
        if self
            .forward_window
            .is_some_and(|window| window.window_id() == closed)
        {
            self.forward_window = None;
            self.forward = None;
            self.forward_key = None;
            self.forward_error = None;
            self.forward_scroll = ScrollHandle::new();
            if self.reply_jump.as_ref().is_some_and(|jump| jump.read_only) {
                self.reply_jump = None;
            }
            changed = true;
        }
        self.video_windows.retain(|_, window| {
            if window.window_id() == closed {
                changed = true;
                false
            } else {
                true
            }
        });
        if changed {
            cx.notify();
        }
    }
    fn desired_window(&self) -> crate::window_chrome::AppWindowKind {
        if matches!(self.phase, Phase::Chat) {
            crate::window_chrome::AppWindowKind::Main
        } else {
            crate::window_chrome::AppWindowKind::Login
        }
    }
    fn sync_desktop_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.desktop_windows
            || self.window_switch_pending
            || self.desired_window() == self.window_kind
        {
            return;
        }
        self.window_switch_pending = true;
        let owner = cx.entity();
        let previous = window.window_handle();
        cx.defer(move |cx| {
            let kind = owner.read(cx).desired_window();
            let old_kind = owner.read(cx).window_kind;
            if kind == old_kind {
                owner.update(cx, |this, _| this.window_switch_pending = false);
                return;
            }
            owner.update(cx, |this, _| this.window_kind = kind);
            let view = owner.clone();
            let result = gpui_kit::open_window(
                crate::window_chrome::app_options(kind, cx),
                cx,
                move |window, cx| {
                    view.update(cx, |this, cx| this.bind_window(window, cx));
                    view
                },
            );
            match result {
                Ok((handle, _)) => {
                    let _ = handle.update(cx, |_, window, cx| {
                        window.activate_window();
                        owner.update(cx, |this, cx| {
                            if kind == crate::window_chrome::AppWindowKind::Login {
                                this.account.update(cx, |input, cx| input.focus(window, cx));
                            }
                            this.window_switch_pending = false;
                            cx.notify();
                        });
                    });
                    let _ = previous.update(cx, |_, window, _| window.remove_window());
                }
                Err(error) => {
                    tracing::error!(?error, "switch application window failed");
                    owner.update(cx, |this, cx| {
                        this.window_kind = old_kind;
                        this.window_switch_pending = false;
                        this.error = Some(error.to_string());
                        cx.notify();
                    });
                }
            }
        });
    }
    fn command(&mut self, command: Command) -> bool {
        if self.commands.try_send(command).is_err() {
            self.error = Some(t!("error.queue").into_owned());
            false
        } else {
            true
        }
    }
    fn mark_recalled(&mut self, conversation: &Conversation, sequence: u64) {
        let conversation = conversation_key(conversation);
        if !self
            .model
            .recalled
            .insert(format!("{conversation}:{sequence}"))
        {
            return;
        }
        if !matches!(self.phase, Phase::Chat)
            || self.page != Page::Chat
            || self.transition.is_some()
            || self.model.selected.as_ref() != Some(&conversation)
        {
            return;
        }
        if let Some(message) = self
            .model
            .messages
            .get(&conversation)
            .and_then(|rows| rows.iter().find(|message| message.sequence == sequence))
        {
            let key = message_key(message);
            if let Some(original) = self.visible_bubbles.borrow().get(&key).copied() {
                self.recall_tweens.insert(
                    key,
                    RecallTween {
                        original,
                        started: self.recall_clock.now(),
                    },
                );
            }
        }
    }
    fn animate_recalls(&mut self, cx: &mut Context<Self>) {
        if cx.reduce_motion()
            || self.page != Page::Chat
            || !matches!(self.phase, Phase::Chat)
            || self.transition.is_some()
        {
            self.recall_tweens.clear();
        }
        let now = cx.background_executor().now();
        self.recall_tweens.retain(|_, tween| {
            !SpringConfig::new(280., 30., 1.).is_settled(tween.state(now), 1., 0.001)
        });
        if self.recall_animating || self.recall_tweens.is_empty() {
            return;
        }
        self.recall_animating = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let now = cx.background_executor().now();
                        this.recall_tweens.retain(|key, tween| {
                            this.visible_bubbles.borrow().contains_key(key)
                                && !SpringConfig::new(280., 30., 1.).is_settled(
                                    tween.state(now),
                                    1.,
                                    0.001,
                                )
                        });
                        this.recall_animating = !this.recall_tweens.is_empty();
                        cx.notify();
                        this.recall_animating
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
            }
        })
        .detach();
    }
    fn apply(&mut self, update: Update) {
        let identity_changed = matches!(
            &update,
            Update::Online(_)
                | Update::Contacts { .. }
                | Update::Restored(_)
                | Update::UserUin { .. }
                | Update::Members { .. }
                | Update::Message(_)
                | Update::CachedMessages { .. }
                | Update::History { .. }
                | Update::ReplyHistory { .. }
                | Update::Sent { .. }
        );
        if matches!(
            &update,
            Update::Connecting | Update::LoginRequired | Update::Challenge(_)
        ) {
            self.confirmation = None;
            self.phone_conversation = false;
            self.send_scroll = None;
            self.transition = None;
            self.recall_tweens.clear();
            self.visible_bubbles.borrow_mut().clear();
            self.scroll_anchor = None;
            self.bounce = Bounce::default();
        }
        let near_bottom = (self.scroll.offset().y + self.scroll.max_offset().y).abs() < px(80.);
        let paginating =
            matches!(&update, Update::History { key, .. } if self.loading_more.contains(key));
        match update {
            Update::Activity(record) => self.apply_activity(record),
            Update::ReactionDone {
                group,
                sequence,
                code,
                result,
            } => {
                self.pending_reactions.remove(&(group, sequence, code));
                match result {
                    Ok(record) => self.apply_activity(record),
                    Err(error) => self.error = Some(error),
                }
            }
            Update::Connecting => self.phase = Phase::Connecting,
            Update::Challenge(challenge) => {
                self.challenge = Some(challenge);
                self.phase = Phase::Login;
            }
            Update::LoginRequired => self.phase = Phase::Login,
            Update::Account(uin) => {
                self.status = if uin > 0 {
                    format!("{uin}")
                } else {
                    String::new()
                };
            }
            Update::Qr { bytes, url } => {
                self.phase = Phase::Login;
                self.qr = Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
                self.qr_url = Some(url);
                self.status = "等待扫码".into();
            }
            Update::Status(status) => {
                if !status.is_empty() {
                    self.status = status;
                }
            }
            Update::Online(profile) => {
                if self
                    .model
                    .profile
                    .as_ref()
                    .is_some_and(|old| old.uin != profile.uin)
                {
                    self.model = Model::default();
                    self.confirmation = None;
                    self.compose_images.clear();
                    self.compose_faces.clear();
                    self.favorites.clear();
                    self.favorites_refresh = None;
                    self.favorites_loading = false;
                    self.emoji_open = false;
                    self.poke_notices.clear();
                    self.pending_pokes.clear();
                    self.pending_reactions.clear();
                    self.pending_copy = None;
                    self.draft_contents.clear();
                    self.transition = None;
                    self.send_scroll = None;
                    self.scroll_anchor = None;
                    self.bounce = Bounce::default();
                    self.loading_more.clear();
                    self.viewer = None;
                    self.forward_cache.clear();
                    self.forward_key = None;
                    self.forward_error = None;
                    self.expanded_images.clear();
                    self.bubble_sizes.borrow_mut().clear();
                    self.lazy_heights.borrow_mut().clear();
                    self.recall_tweens.clear();
                    self.visible_bubbles.borrow_mut().clear();
                    self.fades.clear();
                    self.reply_jump = None;
                    self.reply = None;
                    self.mentions.clear();
                    self.forward = None;
                    self.details = None;
                    self.retries.clear();
                }
                self.requested.retain(|key| !key.starts_with("avatar-uid:"));
                self.challenge = None;
                self.phase = Phase::Chat;
                self.model.profile = Some(profile);
                self.error = None;
                self.status = "在线".into();
            }
            Update::Offline(reason) => {
                self.status = "离线".into();
                self.error = Some(reason);
            }
            Update::Restored(model) => {
                self.transition = None;
                self.send_scroll = None;
                self.recall_tweens.clear();
                self.visible_bubbles.borrow_mut().clear();
                self.scroll_anchor = None;
                self.reply_jump = None;
                self.bounce = Bounce::default();
                self.loading_more.clear();
                let profile = self.model.profile.take();
                self.model = *model;
                self.model.profile = profile;
                self.rebuild_pokes();
                if let Some(chat) = self.model.selected_chat() {
                    self.command(Command::Select(chat.conversation.clone()));
                    self.load_history(false);
                }
            }
            Update::Contacts { friends, groups } => {
                self.model.contacts(friends, groups);
                if let Some(chat) = self.model.selected_chat() {
                    let conversation = chat.conversation.clone();
                    self.command(Command::Select(conversation));
                    self.load_history(false);
                }
                self.persist();
            }
            Update::Members { group, members } => {
                self.model.members.insert(group, members);
            }
            Update::Announcements { group, result } => {
                self.model.announcements.insert(group, result);
            }
            Update::Message(message) => {
                let visible = self.page == Page::Chat && (!self.phone || self.phone_conversation);
                self.model.receive_visible(message, true, visible);
                self.persist();
            }
            Update::CachedMessages { key: _, messages } => {
                for message in messages {
                    self.model.receive(message, false);
                }
            }
            Update::History { key, result } => {
                if self.loading_more.remove(&key)
                    && self.model.selected.as_ref() == Some(&key)
                    && result.is_ok()
                    && self.reply_jump.is_none()
                {
                    self.scroll_anchor = self.capture_timeline_anchor();
                }
                let state = self.model.history.entry(key.clone()).or_default();
                state.loading = false;
                match result {
                    Ok((messages, cursor)) => {
                        state.loaded = true;
                        state.cursor = cursor;
                        state.error = None;
                        for message in messages {
                            self.model.receive(message, false);
                        }
                    }
                    Err(error) => state.error = Some(error),
                }
            }
            Update::ReplyHistory { key, reply, result } => {
                let matching = self.reply_jump.as_ref().is_some_and(|jump| {
                    !jump.read_only
                        && conversation_key(&jump.conversation) == key
                        && jump.reply.sequences == reply.sequences
                        && jump.reply.message_id == reply.message_id
                });
                match result {
                    Ok(messages) => {
                        for message in messages {
                            self.model.receive(message, false);
                        }
                        if matching {
                            self.reply_jump.as_mut().unwrap().waiting = false;
                        }
                    }
                    Err(error) => {
                        if matching {
                            self.reply_jump = None;
                            self.error = Some(error);
                        }
                    }
                }
            }
            Update::Sent { id, result } => {
                let pending = self.model.pending.remove(&id);
                match result {
                    Ok(message) => {
                        self.model.failures.remove(&id);
                        self.retries.remove(&id);
                        self.model.confirm(id, message);
                    }
                    Err(error) => {
                        if let Some((key, text)) = pending {
                            self.model.failures.insert(id, (key, text, error));
                        } else {
                            self.error = Some(error);
                        }
                    }
                }
            }
            Update::Image { key, result } => match result {
                Ok(path) => {
                    self.assets.insert(key, path);
                }
                Err(error) => {
                    if self.pending_copy.as_ref().is_some_and(|copy| {
                        copy.message
                            .elements
                            .iter()
                            .enumerate()
                            .any(|(index, element)| {
                                matches!(element, MessageElement::Image(_))
                                    && image_key(&copy.message, index) == key
                            })
                    }) {
                        self.pending_copy = None;
                        self.error = Some(error.clone());
                    }
                    if let Some(viewer) = &mut self.viewer
                        && viewer.current().key == key
                    {
                        viewer.error = Some(error);
                    }
                }
            },
            Update::Voice { .. } => {}
            Update::Video { key, result } => {
                self.video_results.insert(key, result);
            }
            Update::UserUin { uid, result } => match result {
                Ok(uin) => {
                    self.model.resolved_uins.insert(uid, uin);
                }
                Err(error) => tracing::warn!(%uid, %error, "resolve avatar UIN failed"),
            },
            Update::Asset { key, path } => {
                self.assets.insert(key, path);
            }
            Update::Forward { key, messages } => {
                if self.forward_key.as_ref() == Some(&key) {
                    self.forward = Some(messages.clone());
                    self.forward_error = None;
                }
                self.forward_cache.insert(key, messages);
            }
            Update::ForwardFailed { key, error } => {
                if self.forward_key.as_ref() == Some(&key) {
                    self.forward_error = Some(error);
                }
            }
            Update::Recalled {
                conversation,
                sequence,
            } => {
                self.mark_recalled(&conversation, sequence);
            }
            Update::Favorites { cached, result } => {
                if !cached {
                    self.favorites_loading = false;
                }
                match result {
                    Ok(items) => {
                        self.favorites = items;
                        self.favorites_error = false;
                    }
                    Err(_) => self.favorites_error = true,
                }
            }
            Update::Notification(event) => {
                if matches!(event, PushEvent::Poke { .. } | PushEvent::Reaction { .. }) {
                    self.apply_activity(crate::activity::ActivityRecord::notification(
                        event.clone(),
                    ));
                }
                if let PushEvent::Recall {
                    conversation,
                    sequence,
                    ..
                } = &event
                {
                    self.mark_recalled(conversation, *sequence);
                }
            }
            Update::Error(error) => self.error = Some(error),
        }
        if identity_changed {
            self.rebuild_pokes();
        }
        self.request_assets();
        if near_bottom
            && self.send_scroll.is_none()
            && !paginating
            && self.scroll_anchor.is_none()
            && !self.reply_jump.as_ref().is_some_and(|jump| !jump.read_only)
        {
            self.scroll.scroll_to_bottom();
        }
    }
    fn persist(&mut self) {
        self.command(Command::SaveChats(self.model.chats.clone()));
    }
    fn asset(&mut self, key: String, url: String) {
        if self.requested.contains(&key) || self.assets.contains_key(&key) {
            return;
        }
        if self.command(Command::Asset {
            key: key.clone(),
            url,
        }) {
            self.requested.insert(key);
        }
    }
    fn visible_groups(&self, read_only: bool) -> Vec<&[Message]> {
        let (rows, scroll) = if read_only {
            (self.forward.as_deref(), &self.forward_scroll)
        } else {
            (
                self.model
                    .selected
                    .as_ref()
                    .and_then(|key| self.model.messages.get(key))
                    .map(Vec::as_slice),
                &self.scroll,
            )
        };
        let Some(rows) = rows else {
            return Vec::new();
        };
        let first = scroll.top_item().saturating_sub(2);
        let last = scroll.bottom_item().saturating_add(2);
        message_groups(rows, &self.model.recalled)
            .into_iter()
            .skip(first)
            .take(last.saturating_sub(first) + 1)
            .collect()
    }
    fn request_assets(&mut self) {
        let faces = self
            .visible_groups(false)
            .into_iter()
            .chain(self.visible_groups(true))
            .flat_map(|rows| rows.iter())
            .flat_map(|message| message.elements.iter())
            .filter_map(crate::expressions::Face::from_element)
            .collect::<Vec<_>>();
        for face in faces {
            self.request_face(&face);
        }
        let reaction_faces = self
            .visible_groups(false)
            .into_iter()
            .flatten()
            .flat_map(|message| self.reaction_keys(message))
            .filter_map(|(key, _)| key.2.parse::<u32>().ok())
            .filter(|id| *id < 4096)
            .collect::<Vec<_>>();
        for id in reaction_faces {
            self.request_face(&crate::expressions::Face::new(id));
        }
        let mut avatars = Vec::new();
        if let Some(profile) = &self.model.profile {
            avatars.push(profile.uin);
        }
        if let Some(key) = &self.model.selected
            && let Some(messages) = self.model.messages.get(key)
        {
            for message in messages.iter().rev().take(80) {
                avatars.push(self.model.sender_uin(message));
            }
        }
        if let Some(chat) = self.model.selected_chat() {
            match chat.conversation {
                Conversation::Friend { uin, .. } => avatars.push(uin),
                Conversation::Group { uin } => {
                    if let Some(members) = self.model.members.get(&uin) {
                        avatars.extend(members.iter().take(40).map(|member| member.uin));
                    }
                }
                _ => {}
            }
        }
        let chat_avatars = self
            .model
            .sorted_chats()
            .into_iter()
            .take(80)
            .map(|chat| match chat.conversation {
                Conversation::Group { uin } => (
                    format!("group-avatar:{uin}"),
                    format!("https://p.qlogo.cn/gh/{uin}/{uin}/100"),
                ),
                Conversation::Friend { uin, .. } | Conversation::Temporary { uin, .. } => (
                    format!("avatar:{uin}"),
                    format!("https://q1.qlogo.cn/g?b=qq&nk={uin}&s=100"),
                ),
            })
            .collect::<Vec<_>>();
        for (key, url) in chat_avatars {
            self.asset(key, url);
        }
        let avatar_senders = self
            .forward
            .iter()
            .flatten()
            .take(80)
            .chain(self.visible_groups(false).into_iter().flatten())
            .chain(self.visible_groups(true).into_iter().flatten())
            .map(|message| (self.model.sender_uin(message), message.sender_uid.clone()))
            .collect::<Vec<_>>();
        for (uin, uid) in avatar_senders {
            if uin > 0 {
                avatars.push(uin);
            } else if !uid.is_empty() {
                let key = format!("avatar-uid:{uid}");
                if !self.requested.contains(&key) && self.command(Command::ResolveUser { uid }) {
                    self.requested.insert(key);
                }
            }
        }
        for uin in avatars {
            if uin > 0 {
                self.asset(
                    format!("avatar:{uin}"),
                    format!("https://q1.qlogo.cn/g?b=qq&nk={uin}&s=100"),
                );
            }
        }
        let forward_previews = self
            .model
            .selected
            .as_ref()
            .and_then(|key| self.model.messages.get(key))
            .map(|messages| {
                messages
                    .iter()
                    .rev()
                    .take(30)
                    .flat_map(|message| {
                        message
                            .elements
                            .iter()
                            .enumerate()
                            .filter_map(move |(index, element)| {
                                if let MessageElement::Forward(reference) = element {
                                    Some((
                                        format!(
                                            "{}:{}:{index}",
                                            conversation_key(&message.conversation),
                                            message.sequence
                                        ),
                                        reference.clone(),
                                        matches!(message.conversation, Conversation::Group { .. }),
                                    ))
                                } else {
                                    None
                                }
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (key, reference, is_group) in forward_previews {
            let request_key = format!("forward-preview:{key}");
            if !self.forward_cache.contains_key(&key)
                && !self.requested.contains(&request_key)
                && ferrsphen::viewer::forward_preview(&reference).is_empty()
                && self.command(Command::Forward {
                    key,
                    reference,
                    is_group,
                })
            {
                self.requested.insert(request_key);
            }
        }
        let mut previews =
            self.model
                .selected
                .as_ref()
                .and_then(|key| self.model.messages.get(key))
                .map(|messages| {
                    messages
                        .iter()
                        .rev()
                        .take(50)
                        .flat_map(|message| {
                            message.elements.iter().enumerate().filter_map(
                                move |(index, element)| {
                                    if let MessageElement::Image(reference) = element {
                                        Some((image_key(message, index), reference.clone()))
                                    } else {
                                        None
                                    }
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
        if let Some(messages) = &self.forward {
            previews.extend(messages.iter().take(80).flat_map(|message| {
                message.elements.iter().enumerate().filter_map(
                    move |(index, element)| match element {
                        MessageElement::Image(reference) => {
                            Some((image_key(message, index), reference.clone()))
                        }
                        _ => None,
                    },
                )
            }));
        }
        let visible = self
            .model
            .selected
            .as_ref()
            .and_then(|key| self.model.messages.get(key))
            .into_iter()
            .flatten()
            .rev()
            .take(50)
            .chain(self.visible_groups(false).into_iter().flatten())
            .chain(self.visible_groups(true).into_iter().flatten())
            .collect::<Vec<_>>();
        let videos = visible
            .iter()
            .copied()
            .chain(self.forward.iter().flatten().take(80))
            .flat_map(|message| {
                message
                    .elements
                    .iter()
                    .enumerate()
                    .filter_map(|(index, element)| {
                        if let MessageElement::Video(reference) = element {
                            Some((
                                format!(
                                    "preview:video:{}:{}:{index}",
                                    conversation_key(&message.conversation),
                                    message.sequence
                                ),
                                reference.clone(),
                            ))
                        } else {
                            None
                        }
                    })
            })
            .collect::<Vec<_>>();
        let mut quotes = visible
            .iter()
            .flat_map(|message| {
                message.elements.iter().filter_map(|element| {
                    if let MessageElement::Reply(reply) = element {
                        Some(self.quoted_message(
                            message,
                            reply,
                            self.forward.as_ref().is_some_and(|rows| {
                                rows.iter().any(|row| std::ptr::eq(row, *message))
                            }),
                        ))
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();
        if let Some(reply) = &self.reply {
            quotes.push(reply.clone());
        }
        if let Some(jump) = &self.reply_jump
            && let Some(rows) = self
                .model
                .messages
                .get(&conversation_key(&jump.conversation))
            && let Some(target) = reply_target(rows, &jump.reply)
        {
            for group in message_groups(rows, &self.model.recalled) {
                if group.iter().any(|row| std::ptr::eq(row, target)) {
                    quotes.extend_from_slice(group);
                    break;
                }
            }
        }
        for rows in self
            .model
            .selected
            .as_ref()
            .and_then(|key| self.model.messages.get(key))
            .map(Vec::as_slice)
            .into_iter()
            .chain(self.forward.as_deref())
        {
            for group in message_groups(rows, &self.model.recalled) {
                if self.stack_expanded(group) {
                    quotes.extend_from_slice(group);
                }
            }
        }
        for group in self
            .visible_groups(false)
            .into_iter()
            .chain(self.visible_groups(true))
        {
            if image_only(&group[0]) {
                quotes.extend(
                    group
                        .iter()
                        .take(if group.len() <= 5 { 5 } else { 3 })
                        .cloned(),
                );
            } else {
                quotes.extend_from_slice(group);
            }
        }
        for message in &quotes {
            previews.extend(
                message
                    .elements
                    .iter()
                    .enumerate()
                    .filter_map(|(index, element)| match element {
                        MessageElement::Image(reference) => {
                            Some((image_key(message, index), reference.clone()))
                        }
                        _ => None,
                    }),
            );
        }
        let mut cards = Vec::new();
        for group in self
            .visible_groups(false)
            .into_iter()
            .chain(self.visible_groups(true))
        {
            if !image_only(&group[0]) {
                continue;
            }
            let count = group
                .iter()
                .map(|message| {
                    message
                        .elements
                        .iter()
                        .filter(|element| matches!(element, MessageElement::Image(_)))
                        .count()
                })
                .sum::<usize>();
            for (slot, (message, index, reference)) in group
                .iter()
                .flat_map(|message| {
                    message
                        .elements
                        .iter()
                        .enumerate()
                        .filter_map(move |(index, element)| match element {
                            MessageElement::Image(reference) => Some((message, index, reference)),
                            _ => None,
                        })
                })
                .take(if count <= 5 { 5 } else { 3 })
                .enumerate()
                .skip(1)
            {
                let source = image_key(message, index);
                let size = ferrsphen::card_preview::fitted(reference.width, reference.height);
                let (width, height) = (size.width.round() as u32, size.height.round() as u32);
                let key = card_key(&source, slot, width, height);
                if !self.requested.contains(&key)
                    && let Some(path) = self.assets.get(&source)
                {
                    cards.push((key, path.clone(), slot, width, height));
                }
            }
        }
        for (key, path, slot, width, height) in cards {
            if self.command(Command::CardPreview {
                key: key.clone(),
                path,
                slot,
                width,
                height,
            }) {
                self.requested.insert(key);
            }
        }
        for (key, reference) in videos {
            if !self.requested.contains(&key)
                && self.command(Command::VideoPreview {
                    key: key.clone(),
                    reference,
                })
            {
                self.requested.insert(key);
            }
        }
        for (key, reference) in previews {
            if !self.requested.contains(&key)
                && self.command(Command::Preview {
                    key: key.clone(),
                    reference,
                })
            {
                self.requested.insert(key);
            }
        }
    }
    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(uin) = self.account.read(cx).value().trim().parse::<u64>() else {
            self.error = Some(t!("login.enter_account").into_owned());
            cx.notify();
            return;
        };
        let password = self.password.read(cx).value().to_string();
        if password.is_empty() {
            self.error = Some(t!("login.enter_password").into_owned());
            cx.notify();
            return;
        }
        if self.command(Command::Login { uin, password }) {
            self.password
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.phase = Phase::Connecting;
            self.error = None;
        }
        cx.notify();
    }
    fn select(&mut self, conversation: Conversation, window: &mut Window, cx: &mut Context<Self>) {
        self.emoji_open = false;
        if self.phone {
            let already_selected =
                self.model.selected.as_ref() == Some(&conversation_key(&conversation));
            self.phone_conversation = true;
            self.page = Page::Chat;
            self.transition = None;
            if already_selected {
                if let Some(chat) = self
                    .model
                    .chats
                    .iter_mut()
                    .find(|chat| chat.conversation == conversation)
                {
                    chat.unread = 0;
                }
                self.persist();
                self.request_assets();
            } else {
                self.commit_select(conversation.clone(), window, cx);
            }
            if !cx.reduce_motion() {
                self.transition = Some(ConversationSwitch {
                    destination: conversation,
                    started: cx.background_executor().now(),
                    entering: true,
                });
            }
            window.blur(cx);
            window.dismiss_virtual_keyboard();
            cx.notify();
            return;
        }
        if self.model.selected.as_ref() == Some(&conversation_key(&conversation)) {
            self.transition = None;
            cx.notify();
            return;
        }
        self.send_scroll = None;
        self.recall_tweens.clear();
        self.visible_bubbles.borrow_mut().clear();
        if self.model.selected.is_none() || cx.reduce_motion() {
            self.transition = None;
            self.commit_select(conversation, window, cx);
        } else {
            window.blur(cx);
            if let Some(transition) = self
                .transition
                .as_mut()
                .filter(|transition| !transition.entering)
            {
                transition.destination = conversation;
            } else {
                self.transition = Some(ConversationSwitch {
                    destination: conversation,
                    started: cx.background_executor().now(),
                    entering: false,
                });
            }
            cx.notify();
        }
    }
    fn commit_select(
        &mut self,
        conversation: Conversation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        gpui_kit::base::TextSelection::clear(window, cx);
        self.bounce = Bounce::default();
        self.send_scroll = None;
        self.scroll_anchor = None;
        self.recall_tweens.clear();
        self.visible_bubbles.borrow_mut().clear();
        if let Some(key) = self.model.selected.clone() {
            self.draft_contents
                .insert(key, self.composer.read(cx).content());
        }
        self.model.selected = Some(conversation_key(&conversation));
        if let Some(chat) = self.model.chats.iter_mut().find(|chat| {
            Some(&conversation_key(&chat.conversation)) == self.model.selected.as_ref()
        }) {
            chat.unread = 0;
        }
        let draft = self
            .model
            .selected_chat()
            .map(|chat| chat.draft.clone())
            .unwrap_or_default();
        let content = self
            .draft_contents
            .get(&conversation_key(&conversation))
            .cloned()
            .unwrap_or_else(|| draft.into());
        self.composer.update(cx, |input, cx| {
            input.set_value(content, window, cx);
            if !self.phone {
                input.focus(window, cx);
            }
        });
        self.reply_jump = None;
        self.mentions.clear();
        self.reply = None;
        self.command(Command::Select(conversation));
        self.load_history(false);
        self.persist();
        self.scroll.scroll_to_bottom();
        self.request_assets();
        cx.notify();
    }
    fn load_history(&mut self, more: bool) {
        let Some(chat) = self.model.selected_chat() else {
            return;
        };
        let conversation = chat.conversation.clone();
        let key = conversation_key(&conversation);
        let state = self.model.history.entry(key.clone()).or_default();
        if state.loading
            || (!more && state.loaded)
            || (more && state.loaded && state.cursor.is_none())
        {
            return;
        }
        state.loading = true;
        state.error = None;
        let mut cursor = state.cursor.clone();
        if more {
            if let Some(rows) = self.model.messages.get(&key) {
                cursor = match cursor {
                    Some(ferrsphen::model::Cursor::Group(end)) => {
                        let oldest = rows
                            .iter()
                            .filter_map(|message| {
                                (message.sequence > 0).then_some(message.sequence)
                            })
                            .min();
                        let end =
                            oldest.map_or(end, |sequence| end.min(sequence.saturating_sub(1)));
                        if end == 0 {
                            state.loading = false;
                            state.cursor = None;
                            return;
                        }
                        Some(ferrsphen::model::Cursor::Group(end))
                    }
                    Some(ferrsphen::model::Cursor::Direct { time, random }) => {
                        let oldest = rows
                            .iter()
                            .filter(|message| message.time > 0)
                            .min_by_key(|message| message.time);
                        if let Some(message) =
                            oldest.filter(|message| message.time < i64::from(time))
                        {
                            Some(ferrsphen::model::Cursor::Direct {
                                time: message.time.min(i64::from(u32::MAX)) as u32,
                                random: message.random,
                            })
                        } else {
                            Some(ferrsphen::model::Cursor::Direct { time, random })
                        }
                    }
                    cursor => cursor,
                };
            }
            self.loading_more.insert(key.clone());
        }
        if !self.command(Command::History {
            conversation,
            cursor,
        }) {
            self.loading_more.remove(&key);
            self.model.history.entry(key).or_default().loading = false;
        }
    }
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirmation.is_some() || self.status == "离线" {
            return;
        }
        let Some(chat) = self.model.selected_chat() else {
            return;
        };
        let conversation = chat.conversation.clone();
        let input = self.composer.read(cx);
        let text = input.value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let tokens = input.tokens().to_vec();
        let mut parts = Vec::new();
        let mut cursor = 0;
        for span in tokens {
            let range = span.range();
            if range.start > cursor {
                parts.push(ComposePart::Element(Box::new(MessageElement::Text(
                    text[cursor..range.start].to_owned(),
                ))));
            }
            if let Some(face) = self.compose_faces.get(span.token().id().as_ref()) {
                parts.push(ComposePart::Element(Box::new(face.element())));
                cursor = range.end;
                continue;
            }
            let Some(path) = self.compose_images.get(span.token().id().as_ref()) else {
                self.error = Some(t!("error.image_unavailable").into_owned());
                cx.notify();
                return;
            };
            parts.push(ComposePart::Image(path.clone()));
            cursor = range.end;
        }
        if cursor < text.len() {
            parts.push(ComposePart::Element(Box::new(MessageElement::Text(
                text[cursor..].to_owned(),
            ))));
        }
        let mut prefix = Vec::new();
        for mention in &self.mentions {
            let mut removed = false;
            for part in &mut parts {
                if let ComposePart::Element(element) = part
                    && let MessageElement::Text(text) = element.as_mut()
                {
                    let pattern = format!("@{} ", mention.display);
                    if !removed && text.contains(&pattern) {
                        *text = text.replacen(&pattern, "", 1);
                        removed = true;
                    }
                }
            }
            prefix.push(ComposePart::Element(Box::new(MessageElement::Mention(
                mention.clone(),
            ))));
        }
        if let Some(reply) = self.reply.as_ref() {
            match Reply::with_preview(reply) {
                Ok(reply) => {
                    prefix.push(ComposePart::Element(Box::new(MessageElement::Reply(reply))))
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            }
        }
        prefix.extend(parts);
        let parts = prefix;
        let mut elements = Vec::new();
        let mut images = Vec::new();
        for part in &parts {
            match part {
                ComposePart::Element(element) => elements.push(element.as_ref().clone()),
                ComposePart::Image(path) => {
                    let Ok(target) = conversation.media_conversation() else {
                        return;
                    };
                    let mut reference = atri_qq::media::MediaReference::new(
                        atri_qq::media::MediaKind::Image,
                        target,
                    );
                    reference.name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    if let Ok((width, height)) = image::image_dimensions(path) {
                        reference.width = width;
                        reference.height = height;
                    }
                    images.push((elements.len(), path.clone()));
                    elements.push(MessageElement::Image(reference));
                }
            }
        }
        self.sequence += 1;
        let id = self.sequence;
        let nonce = (rand::random::<u32>() & 0x7fff_ffff).max(1);
        let command = if images.is_empty() {
            Command::Send {
                id,
                nonce,
                conversation: conversation.clone(),
                elements: elements.clone(),
            }
        } else {
            Command::Compose {
                id,
                nonce,
                conversation: conversation.clone(),
                parts,
            }
        };
        if self.command(command.clone()) {
            let staged = self.model.stage(
                id,
                conversation.clone(),
                elements,
                nonce,
                chrono::Utc::now().timestamp(),
            );
            for (index, path) in images {
                self.assets.insert(image_key(&staged, index), path);
            }
            self.reply = None;
            self.reply_jump = None;
            self.scroll_anchor = None;
            self.mentions.clear();
            self.scroll_after_send(&conversation_key(&conversation), cx);
            self.retries.insert(id, command);
            self.model.pending.insert(
                id,
                (conversation_key(&conversation), summary(&staged.elements)),
            );
            self.draft_contents.remove(&conversation_key(&conversation));
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.persist();
        }
        cx.notify();
    }
    fn attach(&mut self, kind: AttachmentKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.model.selected_chat() else {
            return;
        };
        if self.confirmation.is_some() {
            return;
        }
        let conversation = chat.conversation.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: matches!(kind, AttachmentKind::Image),
            prompt: Some(t!("chat.pick_file").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.model.selected.as_ref() != Some(&conversation_key(&conversation)) {
                        return;
                    }
                    if matches!(kind, AttachmentKind::Image) {
                        for path in paths {
                            this.insert_image(path, window, cx);
                        }
                    } else if matches!(kind, AttachmentKind::File | AttachmentKind::Voice) {
                        this.confirmation = Some(AttachmentConfirmation {
                            conversation,
                            items: paths.into_iter().map(|path| (path, kind)).collect(),
                        });
                        this.confirmation_focus.focus(window, cx);
                    } else {
                        for path in paths {
                            this.submit_attachment(conversation.clone(), path, kind, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn paste_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.confirmation.is_some() {
            return false;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        if let Some(paths) = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.clone()),
            _ => None,
        }) {
            self.receive_paths(paths.paths(), window, cx);
            return true;
        }
        let parts = match ferrsphen::rich_clipboard::rich_parts(&item) {
            Ok(Some(parts)) => parts,
            Ok(None) => return false,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return true;
            }
        };
        let files = parts
            .iter()
            .map(|part| ferrsphen::rich_clipboard::materialize(part, self.compose_cache.path()))
            .collect::<anyhow::Result<Vec<_>>>();
        match files {
            Ok(files) => {
                for (part, file) in parts.into_iter().zip(files) {
                    match part {
                        ferrsphen::rich_clipboard::Part::Text { text } => {
                            self.composer
                                .update(cx, |input, cx| input.replace(text, window, cx));
                        }
                        ferrsphen::rich_clipboard::Part::Face { face } => {
                            self.insert_face(face, window, cx)
                        }
                        ferrsphen::rich_clipboard::Part::Image { .. } => {
                            if let Some(path) = file {
                                self.insert_image(path, window, cx);
                            }
                        }
                    }
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
        true
    }
    fn copy_draft(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let input = self.composer.read(cx);
        let range = input.selected_range();
        let tokens = input
            .tokens()
            .iter()
            .filter(|span| span.range().start >= range.start && span.range().end <= range.end)
            .cloned()
            .collect::<Vec<_>>();
        if tokens.is_empty() {
            return false;
        }
        let value = input.value();
        let result = (|| -> anyhow::Result<ClipboardItem> {
            let mut parts = Vec::new();
            let mut cursor = range.start;
            for span in tokens {
                if cursor < span.range().start {
                    parts.push(ferrsphen::rich_clipboard::text(
                        &value[cursor..span.range().start],
                    ));
                }
                if let Some(face) = self.compose_faces.get(span.token().id().as_ref()) {
                    parts.push(ferrsphen::rich_clipboard::Part::Face { face: face.clone() });
                    cursor = span.range().end;
                    continue;
                }
                let path = self
                    .compose_images
                    .get(span.token().id().as_ref())
                    .ok_or_else(|| anyhow::anyhow!(t!("error.image_unavailable")))?;
                parts.push(ferrsphen::rich_clipboard::image_file(path)?);
                cursor = span.range().end;
            }
            if cursor < range.end {
                parts.push(ferrsphen::rich_clipboard::text(&value[cursor..range.end]));
            }
            ferrsphen::rich_clipboard::clipboard(parts)
        })();
        match result {
            Ok(item) => {
                self.pending_copy = None;
                cx.write_to_clipboard(item);
                if cut {
                    self.composer
                        .update(cx, |input, cx| input.replace("", window, cx));
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
        true
    }
    fn copy_message(&mut self, message: Message, cx: &mut Context<Self>) {
        self.pending_copy = None;
        if self.model.is_recalled(&message) {
            cx.write_to_clipboard(ClipboardItem::new_string(
                t!("common.recalled").into_owned(),
            ));
            return;
        }
        for (index, element) in message.elements.iter().enumerate() {
            if let MessageElement::Image(reference) = element {
                let key = image_key(&message, index);
                if !self.assets.contains_key(&key) {
                    self.command(Command::ViewImage {
                        key,
                        reference: reference.clone(),
                    });
                }
            }
        }
        self.pending_copy = Some(PendingCopy {
            message,
            baseline: cx.read_from_clipboard(),
        });
        self.finish_copy(cx);
    }
    fn finish_copy(&mut self, cx: &mut Context<Self>) {
        let Some(copy) = &self.pending_copy else {
            return;
        };
        if copy
            .message
            .elements
            .iter()
            .enumerate()
            .any(|(index, element)| {
                matches!(element, MessageElement::Image(_))
                    && !self.assets.contains_key(&image_key(&copy.message, index))
            })
        {
            return;
        }
        let copy = self.pending_copy.take().unwrap();
        if cx.read_from_clipboard() != copy.baseline {
            return;
        }
        let result = (|| -> anyhow::Result<ClipboardItem> {
            let parts = copy
                .message
                .elements
                .iter()
                .enumerate()
                .map(|(index, element)| {
                    if matches!(element, MessageElement::Image(_)) {
                        ferrsphen::rich_clipboard::image_file(
                            &self.assets[&image_key(&copy.message, index)],
                        )
                    } else if let Some(face) = crate::expressions::Face::from_element(element) {
                        Ok(ferrsphen::rich_clipboard::Part::Face { face })
                    } else {
                        Ok(ferrsphen::rich_clipboard::text(summary(
                            std::slice::from_ref(element),
                        )))
                    }
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            ferrsphen::rich_clipboard::clipboard(parts)
        })();
        match result {
            Ok(item) => cx.write_to_clipboard(item),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn request_face(&mut self, face: &crate::expressions::Face) {
        if let Some(url) = face.image_url() {
            self.asset(format!("face:{}", face.id), url);
        }
    }
    fn face_image(&self, face: &crate::expressions::Face) -> AnyElement {
        face_image(face, &self.assets)
    }
    fn insert_face(
        &mut self,
        face: crate::expressions::Face,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_face(&face);
        let id = format!("compose-face:{:016x}", rand::random::<u64>());
        let token = gpui_kit::component::input::InlineToken::new(id.clone(), "\u{fffc}")
            .with_label(face.text());
        self.compose_faces.insert(id, face);
        self.emoji_open = false;
        let result = self.composer.update(cx, |input, cx| {
            input.focus(window, cx);
            input.replace_with_token(token, window, cx)
        });
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
        cx.notify();
    }
    fn load_favorites(&mut self) {
        if self.favorites_loading {
            return;
        }
        self.requested.retain(|key| !key.starts_with("favorite:"));
        self.favorites_loading = self.command(Command::Favorites);
        self.favorites_error = !self.favorites_loading;
    }
    fn favorites_visible(&self) -> bool {
        self.emoji_open
            && self.emoji_favorites
            && matches!(self.phase, Phase::Chat)
            && self.page == Page::Chat
            && (!self.phone || self.phone_conversation)
            && self.model.selected_chat().is_some()
    }
    fn sync_favorites_refresh(&mut self, cx: &mut Context<Self>) {
        if !self.favorites_visible() {
            self.favorites_refresh = None;
            return;
        }
        if self.favorites_refresh.is_some() {
            return;
        }
        self.favorites_refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if !this.favorites_visible() {
                            return false;
                        }
                        this.load_favorites();
                        cx.notify();
                        true
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
            }
        }));
    }
    fn expression_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let owner = cx.entity();
        let change_owner = owner.clone();
        Popover::new("expression-picker")
            .anchor(Anchor::BottomLeft)
            .open(self.emoji_open)
            .on_open_change(move |open, _, cx| {
                change_owner.update(cx, |this, cx| {
                    this.emoji_open = *open;
                    if *open {
                        for &(id, _) in crate::expressions::FACES {
                            this.request_face(&crate::expressions::Face::new(id));
                        }
                        this.load_favorites();
                    }
                    this.sync_favorites_refresh(cx);
                    cx.notify();
                });
            })
            .trigger(
                Button::new("emoji-button")
                    .ghost()
                    .small()
                    .icon(gpui_kit::assets::IconName::FaceSlightlySmiling)
                    .size(px(if self.phone { 44. } else { 32. }))
                    .tooltip(t!("expressions.title"))
                    .accessibility_label(t!("expressions.title"))
                    .debug_selector(|| "emoji-button".into()),
            )
            .content(move |_, _, cx| owner.update(cx, |this, cx| this.expression_content(cx)))
            .into_any_element()
    }
    fn expression_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = div()
            .flex()
            .gap_1()
            .child(
                Button::new("faces-tab")
                    .debug_selector(|| "faces-tab".into())
                    .ghost()
                    .small()
                    .label(t!("expressions.title"))
                    .selected(!self.emoji_favorites)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emoji_favorites = false;
                        this.sync_favorites_refresh(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("favorites-tab")
                    .debug_selector(|| "favorites-tab".into())
                    .ghost()
                    .small()
                    .label(t!("expressions.favorites"))
                    .selected(self.emoji_favorites)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emoji_favorites = true;
                        this.load_favorites();
                        this.sync_favorites_refresh(cx);
                        cx.notify();
                    })),
            );
        let content = if self.emoji_favorites {
            if self.favorites.is_empty() {
                div()
                    .h(px(230.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if self.favorites_loading {
                        Spinner::new().into_any_element()
                    } else {
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.favorites_error {
                                t!("common.load_failed")
                            } else {
                                t!("expressions.empty")
                            })
                            .into_any_element()
                    })
                    .into_any_element()
            } else {
                let owner = cx.entity().downgrade();
                uniform_list(
                    "favorite-expressions",
                    self.favorites.len().div_ceil(5),
                    move |range, _, cx| {
                        owner
                            .update(cx, |this, cx| {
                                range
                                    .map(|row| {
                                        let items = this
                                            .favorites
                                            .iter()
                                            .skip(row * 5)
                                            .take(5)
                                            .cloned()
                                            .collect::<Vec<_>>();
                                        let mut cells = Vec::new();
                                        for item in items {
                                            let key = format!(
                                                "favorite:{}:{}",
                                                this.model
                                                    .profile
                                                    .as_ref()
                                                    .map_or(0, |user| user.uin),
                                                item.id
                                            );
                                            this.asset(key.clone(), item.url);
                                            let path = this.assets.get(&key).cloned();
                                            let mut button =
                                                Button::new(SharedString::from(key.clone()))
                                                    .debug_selector(move || key.clone())
                                                    .ghost()
                                                    .size(px(52.))
                                                    .disabled(path.is_none())
                                                    .accessibility_label(t!(
                                                        "expressions.favorite"
                                                    ));
                                            if let Some(path) = path {
                                                button = button
                                                    .child(
                                                        img(path.clone())
                                                            .size(px(46.))
                                                            .object_fit(ObjectFit::Contain),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, window, cx| {
                                                            this.emoji_open = false;
                                                            this.insert_favorite(&path, window, cx);
                                                        },
                                                    ));
                                            } else {
                                                button = button.child(Spinner::new());
                                            }
                                            cells.push(button);
                                        }
                                        div()
                                            .flex()
                                            .h(px(56.))
                                            .gap_1()
                                            .children(cells)
                                            .into_any_element()
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    },
                )
                .h(px(230.))
                .w_full()
                .into_any_element()
            }
        } else {
            let rows = crate::expressions::FACES.chunks(6).map(|row| {
                div().flex().gap_1().children(row.iter().map(|&(id, _)| {
                    let face = crate::expressions::Face::new(id);
                    Button::new(SharedString::from(format!("face-choice:{id}")))
                        .debug_selector(move || format!("face-choice:{id}"))
                        .ghost()
                        .size(px(43.))
                        .accessibility_label(face.name.clone())
                        .child(self.face_image(&face))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.insert_face(face.clone(), window, cx)
                        }))
                }))
            });
            div()
                .id("face-list")
                .h(px(230.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };
        div()
            .debug_selector(|| "emoji-panel".into())
            .w(px(288.))
            .flex()
            .flex_col()
            .gap_2()
            .child(tabs)
            .child(content)
            .into_any_element()
    }
    fn participant_avatar(
        &self,
        uin: u64,
        uid: &str,
        name: &str,
        conversation: &Conversation,
        read_only: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let owner = cx.entity().downgrade();
        let uid = uid.to_owned();
        let name = name.to_owned();
        let conversation = conversation.clone();
        let disabled = read_only || self.status == "离线" || (uin == 0 && uid.is_empty());
        div()
            .id(SharedString::from(format!("participant:{uin}:{uid}")))
            .debug_selector(move || format!("participant-avatar:{uin}"))
            .child(self.avatar(uin, &name, cx))
            .context_menu(move |menu, _, _| {
                let mention_owner = owner.clone();
                let poke_owner = owner.clone();
                let mention_uid = uid.clone();
                let mention_name = name.clone();
                let poke_uid = uid.clone();
                let conversation = conversation.clone();
                menu.when(matches!(conversation, Conversation::Group { .. }), |menu| {
                    menu.item(PopupMenuItem::new("@").disabled(disabled).on_click(
                        move |_, window, cx| {
                            let _ = mention_owner.update(cx, |this, cx| {
                                this.mentions.push(Mention {
                                    uin,
                                    uid: Some(mention_uid.clone()),
                                    display: mention_name.clone(),
                                    is_everyone: false,
                                });
                                this.composer.update(cx, |input, cx| {
                                    input.focus(window, cx);
                                    input.replace(format!("@{mention_name} "), window, cx);
                                });
                                cx.notify();
                            });
                        },
                    ))
                })
                .item(
                    PopupMenuItem::new(t!("chat.poke"))
                        .disabled(
                            disabled || matches!(conversation, Conversation::Temporary { .. }),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = poke_owner.update(cx, |this, _| {
                                this.command(Command::Poke {
                                    conversation: conversation.clone(),
                                    target: uin,
                                    uid: poke_uid.clone(),
                                });
                            });
                        }),
                )
            })
            .into_any_element()
    }
    fn receive_poke(
        &mut self,
        group: Option<u64>,
        actor: Option<u64>,
        target: Option<u64>,
        parameters: &std::collections::BTreeMap<String, String>,
        time: i64,
    ) -> bool {
        let own = self.model.profile.as_ref().map_or(0, |profile| profile.uin);
        let key = if let Some(group) = group {
            conversation_key(&Conversation::Group { uin: group })
        } else {
            let peer = [actor, target]
                .into_iter()
                .flatten()
                .find(|uin| *uin != own && *uin != 0);
            let Some(peer) = peer else {
                return false;
            };
            let Some(chat) = self.model.chats.iter().find(|chat| {
                let Conversation::Friend { uin, uid } = &chat.conversation else {
                    return false;
                };
                *uin == peer
                    || (!uid.is_empty()
                        && (self.model.resolved_uins.get(uid) == Some(&peer)
                            || self
                                .model
                                .friends
                                .iter()
                                .any(|friend| friend.uin == peer && friend.uid == *uid)
                            || self
                                .model
                                .messages
                                .get(&conversation_key(&chat.conversation))
                                .is_some_and(|messages| {
                                    messages.iter().any(|message| {
                                        (message.sender_uin == peer && message.sender_uid == *uid)
                                            || (message.recipient_uin == peer
                                                && message.recipient_uid == *uid)
                                    })
                                })))
            }) else {
                return false;
            };
            conversation_key(&chat.conversation)
        };
        let display = |uin: Option<u64>| {
            let Some(uin) = uin.filter(|uin| *uin != 0) else {
                return t!("poke.peer").into_owned();
            };
            if uin == own {
                return t!("poke.you").into_owned();
            }
            let friend = self.model.friends.iter().find(|friend| friend.uin == uin);
            let member = group
                .and_then(|group| self.model.members.get(&group))
                .and_then(|members| members.iter().find(|member| member.uin == uin));
            let fallback = self
                .model
                .chats
                .iter()
                .find(|chat| conversation_key(&chat.conversation) == key)
                .filter(|chat| group.is_none() && !chat.title.starts_with("dm:"))
                .map_or("", |chat| chat.title.as_str());
            ferrsphen::model::display_name(friend, member, fallback, uin)
        };
        let default_action = t!("poke.action");
        let action = parameters
            .get("action_str")
            .filter(|action| !action.trim().is_empty())
            .map_or(default_action.as_ref(), String::as_str);
        let suffix = parameters.get("suffix_str").map_or("", String::as_str);
        let text = format!("{} {action} {}{suffix}", display(actor), display(target));
        let after = self
            .model
            .messages
            .get(&key)
            .and_then(|rows| {
                rows.iter()
                    .rev()
                    .find(|message| message.time.saturating_mul(1000) <= time)
            })
            .map(message_key);
        let notices = self.poke_notices.entry(key).or_default();
        notices.push((after, text));
        if notices.len() > 100 {
            notices.remove(0);
        }
        true
    }
    fn apply_activity(&mut self, record: crate::activity::ActivityRecord) {
        if !self.model.apply_activity(record.clone()) {
            return;
        }
        self.display_poke(record);
    }
    fn display_poke(&mut self, record: crate::activity::ActivityRecord) {
        if let crate::activity::ActivityEvent::Notification(PushEvent::Poke {
            group_uin,
            actor_uin,
            target_uin,
            parameters,
            ..
        }) = &record.event
            && !self.receive_poke(*group_uin, *actor_uin, *target_uin, parameters, record.time)
        {
            self.pending_pokes.push(record);
        }
    }
    fn rebuild_pokes(&mut self) {
        self.poke_notices.clear();
        self.pending_pokes.clear();
        let pokes = self
            .model
            .activity
            .iter()
            .filter(|record| {
                matches!(
                    record.event,
                    crate::activity::ActivityEvent::Notification(PushEvent::Poke { .. })
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        for record in pokes {
            self.display_poke(record);
        }
    }
    fn poke_rows(
        &self,
        messages: &[Message],
        read_only: bool,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        if read_only {
            return vec![];
        }
        let Some(first) = messages.first() else {
            return vec![];
        };
        self.poke_notices
            .get(&conversation_key(&first.conversation))
            .into_iter()
            .flatten()
            .filter(|(after, _)| match after {
                Some(key) => messages.iter().any(|message| message_key(message) == *key),
                None => self
                    .model
                    .messages
                    .get(&conversation_key(&first.conversation))
                    .and_then(|rows| rows.first())
                    .is_some_and(|message| message_key(message) == message_key(first)),
            })
            .map(|(_, text)| {
                div()
                    .debug_selector(|| "poke-notice".into())
                    .w_full()
                    .py_2()
                    .text_center()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(text.clone())
                    .into_any_element()
            })
            .collect()
    }

    fn insert_favorite(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = ferrsphen::rich_clipboard::image_file(path).and_then(|part| {
            ferrsphen::rich_clipboard::materialize(&part, self.compose_cache.path())
        });
        match result {
            Ok(Some(path)) => self.insert_image(path, window, cx),
            Ok(None) => {}
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn insert_image(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !path.is_file() || image::image_dimensions(&path).is_err() {
            self.error = Some(t!("error.read_image").into_owned());
            cx.notify();
            return;
        }
        let id = format!("compose-image:{:016x}", rand::random::<u64>());
        self.compose_images.insert(id.clone(), path);
        let token = gpui_kit::component::input::InlineToken::new(id, "\u{fffc}")
            .with_label(t!("common.image"));
        let result = self.composer.update(cx, |input, cx| {
            input.focus(window, cx);
            input.replace_with_token(token, window, cx)
        });
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
        cx.notify();
    }
    fn receive_paths(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.model.selected_chat() else {
            return;
        };
        if self.confirmation.is_some() {
            return;
        }
        let conversation = chat.conversation.clone();
        let mut standalone = Vec::new();
        for path in paths {
            if !path.is_file() {
                self.error = Some(t!("chat.choose_file").into_owned());
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            match extension.as_str() {
                "png" | "jpg" | "jpeg" | "gif" | "webp" => {
                    self.insert_image(path.clone(), window, cx)
                }
                "silk" | "amr" | "wav" | "mp3" | "ogg" | "opus" | "m4a" | "flac" | "aac" => {
                    standalone.push((path.clone(), AttachmentKind::Voice))
                }
                _ => standalone.push((path.clone(), AttachmentKind::File)),
            }
        }
        if !standalone.is_empty() {
            self.confirmation = Some(AttachmentConfirmation {
                conversation,
                items: standalone,
            });
            self.confirmation_focus.focus(window, cx);
        }
        cx.notify();
    }
    fn submit_attachment(
        &mut self,
        conversation: Conversation,
        path: PathBuf,
        kind: AttachmentKind,
        cx: &mut Context<Self>,
    ) {
        self.sequence += 1;
        let id = self.sequence;
        let text = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let nonce = (rand::random::<u32>() & 0x7fff_ffff).max(1);
        let command = Command::Attachment {
            id,
            nonce,
            conversation: conversation.clone(),
            path: path.clone(),
            kind,
        };
        if self.command(command.clone()) {
            let media_kind = match kind {
                AttachmentKind::Image => atri_qq::media::MediaKind::Image,
                AttachmentKind::Voice => atri_qq::media::MediaKind::Voice,
                AttachmentKind::Video => atri_qq::media::MediaKind::Video,
                AttachmentKind::File => atri_qq::media::MediaKind::File,
            };
            let Ok(target) = conversation.media_conversation() else {
                return;
            };
            let mut reference = atri_qq::media::MediaReference::new(media_kind, target);
            reference.name = text.clone();
            let element = match kind {
                AttachmentKind::Image => MessageElement::Image(reference),
                AttachmentKind::Voice => MessageElement::Voice(reference),
                AttachmentKind::Video => MessageElement::Video(reference),
                AttachmentKind::File => MessageElement::File(reference),
            };
            let staged = self.model.stage(
                id,
                conversation.clone(),
                vec![element],
                nonce,
                chrono::Utc::now().timestamp(),
            );
            if matches!(kind, AttachmentKind::Image) {
                self.assets.insert(image_key(&staged, 0), path);
            }
            self.scroll_after_send(&conversation_key(&conversation), cx);
            self.retries.insert(id, command);
            self.model
                .pending
                .insert(id, (conversation_key(&conversation), text));
        }
        cx.notify();
    }
    fn confirm_attachments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status == "离线" {
            return;
        }
        if let Some(confirmation) = self.confirmation.take() {
            for (path, kind) in confirmation.items {
                self.submit_attachment(confirmation.conversation.clone(), path, kind, cx);
            }
        }
        self.composer
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }
    fn confirmation_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(confirmation) = &self.confirmation else {
            return div().into_any_element();
        };
        let title = if confirmation
            .items
            .iter()
            .all(|(_, kind)| *kind == AttachmentKind::Voice)
        {
            t!("chat.send_voice")
        } else {
            t!("chat.send_file")
        };
        div()
            .id("attachment-confirmation")
            .debug_selector(|| "attachment-confirmation".into())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::rgba(0x00000066))
            .track_focus(&self.confirmation_focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.confirmation = None;
                    this.composer
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .w_96()
                    .max_w(relative(0.94))
                    .max_h(px(480.))
                    .p_5()
                    .gap_4()
                    .flex()
                    .flex_col()
                    .rounded_lg()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when_some(
                        self.model
                            .chats
                            .iter()
                            .find(|chat| chat.conversation == confirmation.conversation),
                        |view, chat| {
                            view.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(chat.title.clone()),
                            )
                        },
                    )
                    .child(
                        div()
                            .id("confirmation-files")
                            .overflow_y_scroll()
                            .max_h(px(280.))
                            .children(confirmation.items.iter().map(|(path, _)| {
                                div().py_1().text_sm().truncate().child(
                                    path.file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned(),
                                )
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("attachment-cancel")
                                    .ghost()
                                    .label(t!("common.cancel"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirmation = None;
                                        this.composer
                                            .update(cx, |input, cx| input.focus(window, cx));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "attachment-confirm-button".into())
                                    .child(
                                        Button::new("attachment-confirm")
                                            .primary()
                                            .label(t!("common.send"))
                                            .disabled(self.status == "离线")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.confirm_attachments(window, cx)
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn avatar(&self, uin: u64, name: &str, cx: &App) -> AnyElement {
        self.avatar_for(&format!("avatar:{uin}"), name, cx)
    }
    fn avatar_for(&self, key: &str, name: &str, cx: &App) -> AnyElement {
        let avatar_key = key.to_owned();
        let base = div()
            .debug_selector(move || avatar_key.clone())
            .flex()
            .items_center()
            .justify_center()
            .size_9()
            .flex_shrink_0()
            .rounded_full()
            .overflow_hidden()
            .bg(cx.theme().muted);
        match self.assets.get(key) {
            Some(path) => base
                .child(
                    img(path.clone())
                        .size_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element(),
            None => base
                .text_sm()
                .child(name.chars().next().unwrap_or('·').to_string())
                .into_any_element(),
        }
    }
    fn login_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if matches!(self.phase, Phase::Connecting) {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t!("login.signing_in"))
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_4()
            .when(self.phone, |view| view.p_4())
            .when(!self.phone, |view| view.p_8())
            .child(
                div()
                    .flex()
                    .items_center()
                    .when(self.phone, |view| view.flex_col().gap_4())
                    .when(!self.phone, |view| view.gap_10())
                    .when(!self.phone || self.phone_qr_login, |view| {
                        view.child(self.qr_login(cx))
                    })
                    .when(!self.phone || !self.phone_qr_login, |view| {
                        view.child(self.password_login(cx))
                    }),
            )
            .when(self.phone, |view| {
                view.child(
                    Button::new("switch-login")
                        .debug_selector(|| "switch-login".into())
                        .ghost()
                        .label(if self.phone_qr_login {
                            t!("login.password_login")
                        } else {
                            t!("login.qr_login")
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.phone_qr_login = !this.phone_qr_login;
                            window.blur(cx);
                            window.dismiss_virtual_keyboard();
                            if this.phone_qr_login {
                                this.qr = None;
                                this.qr_url = None;
                                this.command(Command::Qr);
                            }
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
    fn qr_login(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "qr-login".into())
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(rems(15.))
                    .p_3()
                    .bg(cx.theme().primary_foreground)
                    .rounded_lg()
                    .map(|element| match &self.qr {
                        Some(image) => element.child(img(image.clone()).size_full()),
                        None => element.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("login.qr_loading")),
                        ),
                    }),
            )
            .child(
                Button::new("refresh-qr")
                    .ghost()
                    .label(t!("login.qr_refresh"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.qr = None;
                        this.command(Command::Qr);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn password_login(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "password-login".into())
            .flex()
            .flex_col()
            .w_64()
            .gap_4()
            .child(div().text_lg().child(t!("login.title")))
            .child(Input::new(&self.account).aria_label(t!("login.account")))
            .child(Input::new(&self.password).aria_label(t!("login.password")))
            .child(
                Button::new("login")
                    .primary()
                    .w_full()
                    .label(t!("login.sign_in"))
                    .on_click(cx.listener(|this, _, window, cx| this.login(window, cx))),
            )
            .into_any_element()
    }
    fn challenge_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(challenge) = &self.challenge else {
            return div().into_any_element();
        };
        let (url, captcha) = match challenge {
            atri_qq::LoginChallenge::Captcha { url } => (url.clone(), true),
            atri_qq::LoginChallenge::NewDevice { url } => (url.clone(), false),
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .p_4()
            .child(
                Button::new("verify-link")
                    .ghost()
                    .label(t!("login.open_verification"))
                    .on_click(move |_, _, _| {
                        let _ = open::that(&url);
                    }),
            )
            .when(captcha, |view| {
                view.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(Input::new(&self.ticket).aria_label("ticket"))
                        .child(Input::new(&self.captcha_random).aria_label("randstr")),
                )
                .child(
                    Button::new("submit-verification")
                        .primary()
                        .label(t!("login.submit_verification"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let ticket = this.ticket.read(cx).value().to_string();
                            let random = this.captcha_random.read(cx).value().to_string();
                            if ticket.is_empty() || random.is_empty() {
                                this.error = Some(t!("login.enter_verification").into_owned());
                            } else {
                                this.command(Command::SubmitCaptcha { ticket, random });
                            }
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    fn is_group(&self) -> bool {
        self.model
            .selected_chat()
            .is_some_and(|chat| matches!(chat.conversation, Conversation::Group { .. }))
    }
    fn topbar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let (profile_name, uin) = self
            .model
            .profile
            .as_ref()
            .map(|profile| (profile.nickname.clone(), profile.uin))
            .unwrap_or_default();
        ferrsphen::window_chrome::title_bar(cx)
            .child(
                div()
                    .w_72()
                    .flex_shrink_0()
                    .pl(ferrsphen::window_chrome::leading_inset(window))
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.avatar(uin, &profile_name, cx))
                    .child(div().min_w_0().text_sm().truncate().child(profile_name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_1(),
            )
            .when(self.page == Page::Chat && self.is_group(), |bar| {
                bar.child(
                    Button::new("toggle-sidebar")
                        .ghost()
                        .icon(if self.sidebar {
                            gpui_kit::assets::IconName::PanelRightClose
                        } else {
                            gpui_kit::assets::IconName::PanelRightOpen
                        })
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .accessibility_label(t!("group.details"))
                        .tooltip(t!("group.details"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar = !this.sidebar;
                            cx.notify();
                        })),
                )
            })
            .child(div().w_3())
            .into_any_element()
    }
    fn main_page(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let body = div().flex().flex_1().min_h_0().min_w_0();
        let page = if self.page == Page::Settings {
            div()
                .debug_selector(|| "settings-page".into())
                .flex_1()
                .h_full()
                .into_any_element()
        } else if self.phone {
            if self.phone_conversation {
                self.conversation_panel(window, cx)
            } else {
                self.chat_list(cx)
            }
        } else {
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .child(self.chat_list(cx))
                .child(self.conversation_panel(window, cx))
                .into_any_element()
        };
        if self.phone {
            body.flex_col()
                .child(div().flex().flex_1().min_h_0().min_w_0().child(page))
                .when(
                    self.page == Page::Settings || !self.phone_conversation,
                    |body| body.child(self.navigation(cx)),
                )
                .into_any_element()
        } else {
            body.child(self.navigation(cx))
                .child(page)
                .into_any_element()
        }
    }

    fn show_chat_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        gpui_kit::base::TextSelection::clear(window, cx);
        self.phone_conversation = false;
        self.transition = None;
        self.send_scroll = None;
        self.reply_jump = None;
        self.scroll_anchor = None;
        self.bounce = Bounce::default();
        self.recall_tweens.clear();
        self.visible_bubbles.borrow_mut().clear();
        window.blur(cx);
        window.dismiss_virtual_keyboard();
        cx.notify();
    }

    fn navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::assets::IconName;
        div()
            .debug_selector(|| "navigation".into())
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap_2()
            .when(self.phone, |nav| {
                nav.w_full().h(px(56.)).justify_around().border_t_1()
            })
            .when(!self.phone, |nav| {
                nav.flex_col().w(px(52.)).h_full().py_3().border_r_1()
            })
            .border_color(cx.theme().border)
            .children(
                [
                    (
                        Page::Chat,
                        "nav-chat",
                        t!("chat.title"),
                        IconName::MessageCircle,
                    ),
                    (
                        Page::Settings,
                        "nav-settings",
                        t!("settings.title"),
                        IconName::Settings,
                    ),
                ]
                .into_iter()
                .map(|(page, id, label, icon)| {
                    div().debug_selector(move || id.into()).child(
                        Button::new(id)
                            .ghost()
                            .icon(icon)
                            .when(self.phone, |button| button.size(px(44.)))
                            .tooltip(label.clone())
                            .accessibility_label(label)
                            .when(self.page == page, |button| button.bg(cx.theme().accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.page != page {
                                    this.send_scroll = None;
                                    this.recall_tweens.clear();
                                    this.visible_bubbles.borrow_mut().clear();
                                }
                                this.page = page;
                                if this.phone {
                                    this.show_chat_list(window, cx);
                                }
                                window.blur(cx);
                                cx.notify();
                            })),
                    )
                }),
            )
            .into_any_element()
    }

    fn chat_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search.read(cx).value().to_lowercase();
        let chats = self
            .model
            .sorted_chats()
            .into_iter()
            .filter(|chat| self.search_index.borrow_mut().matches(&chat.title, &query))
            .cloned()
            .collect::<Vec<_>>();
        let owner = cx.entity();
        let rows = uniform_list("chats", chats.len(), move |range, _, cx| {
            owner.update(cx, |this, cx| {
                range
                    .map(|index| this.chat_row(&chats[index], cx))
                    .collect::<Vec<_>>()
            })
        })
        .flex_1()
        .min_h_0()
        .px_1();
        div()
            .debug_selector(|| "chat-list".into())
            .flex()
            .flex_col()
            .when(self.phone, |list| list.w_full().min_w_0().flex_1())
            .when(!self.phone, |list| list.w_72())
            .h_full()
            .flex_shrink_0()
            .when(!self.phone, |list| list.border_r_1())
            .border_color(cx.theme().border)
            .when(self.phone, |list| {
                list.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("chat.title")),
                )
            })
            .child(
                div()
                    .p_3()
                    .child(Input::new(&self.search).aria_label(t!("chat.search_conversations"))),
            )
            .child(rows)
            .into_any_element()
    }
    fn chat_row(&self, chat: &ferrsphen::model::Chat, cx: &mut Context<Self>) -> AnyElement {
        let key = conversation_key(&chat.conversation);
        let selected = self
            .transition
            .as_ref()
            .map(|transition| conversation_key(&transition.destination))
            .as_ref()
            .or(self.model.selected.as_ref())
            == Some(&key);
        let conversation = chat.conversation.clone();
        let preview = self.model.chat_preview(&chat.conversation);
        let uin = match conversation {
            Conversation::Friend { uin, .. } => uin,
            _ => 0,
        };
        Button::new(SharedString::from(format!("chat:{key}")))
            .debug_selector({
                let key = key.clone();
                move || format!("chat:{key}")
            })
            .ghost()
            .selected(selected)
            .when(selected, |button| button.bg(cx.theme().accent))
            .w_full()
            .h(px(72.))
            .p_3()
            .accessibility_label(t!("chat.open", name = chat.title))
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .child(match chat.conversation {
                        Conversation::Group { uin } => {
                            self.avatar_for(&format!("group-avatar:{uin}"), &chat.title, cx)
                        }
                        _ => self.avatar(uin, &chat.title, cx),
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(chat.title.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(preview),
                            ),
                    )
                    .when(chat.unread > 0, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().primary)
                                .child(chat.unread.to_string()),
                        )
                    }),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.select(conversation.clone(), window, cx)
                }),
            )
            .into_any_element()
    }
    fn element(
        &self,
        message: &Message,
        index: usize,
        element: &MessageElement,
        cx: &mut Context<Self>,
        read_only: bool,
    ) -> AnyElement {
        let key = format!(
            "{}:{}:{index}",
            conversation_key(&message.conversation),
            message.sequence
        );
        match element {
            MessageElement::Text(text) => div()
                .debug_selector(move || format!("message-text:{key}"))
                .text_sm()
                .cursor_text()
                .child(crate::message_text::MessageText::new(
                    "message-text",
                    text.clone(),
                ))
                .into_any_element(),
            MessageElement::Mention(mention) => div()
                .text_sm()
                .text_color(cx.theme().primary)
                .cursor_text()
                .child(crate::message_text::MessageText::new(
                    "message-mention",
                    format!("@{}", mention.display),
                ))
                .into_any_element(),
            MessageElement::Reply(reply) => {
                let preview = self.quoted_message(message, reply, read_only);
                let conversation = message.conversation.clone();
                let reply = reply.clone();
                div()
                    .id(SharedString::from(format!("quoted:{key}")))
                    .cursor_pointer()
                    .child(self.reply_card(&preview, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.jump_to_reply(conversation.clone(), reply.clone(), read_only, cx);
                    }))
                    .into_any_element()
            }
            MessageElement::Image(reference) => {
                let reference = reference.clone();
                let viewer_key = image_key(message, index);
                let preview = self.assets.get(&image_key(message, index)).cloned();
                Button::new(SharedString::from(format!("image:{key}")))
                    .ghost()
                    .h_auto()
                    .p_0()
                    .accessibility_label(t!("image.open"))
                    .child(match preview {
                        Some(path) => img(path)
                            .id(SharedString::from(format!("animation:{key}")))
                            .when(reference.width > 0 && reference.height > 0, |image| {
                                let factor = (384. / reference.width as f32)
                                    .min(352. / reference.height as f32)
                                    .min(1.);
                                image
                                    .w(px(reference.width as f32 * factor))
                                    .h(px(reference.height as f32 * factor))
                            })
                            .max_w(rems(24.))
                            .max_h(rems(22.))
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        None => div()
                            .w_48()
                            .h_24()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(t!("common.image"))
                            .into_any_element(),
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_image(viewer_key.clone(), reference.clone(), window, cx);
                        cx.notify();
                    }))
                    .context_menu(self.message_menu(message, cx, read_only, Some(index)))
                    .into_any_element()
            }
            MessageElement::Video(reference) => {
                let reference = reference.clone();
                let video_key = format!("video:{key}");
                let preview = self.assets.get(&format!("preview:{video_key}")).cloned();
                let size = ferrsphen::card_preview::fitted(reference.width, reference.height);
                div()
                    .id(SharedString::from(video_key.clone()))
                    .debug_selector({
                        let key = video_key.clone();
                        move || key.clone()
                    })
                    .relative()
                    .w(px(size.width))
                    .h(px(size.height))
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(gpui_kit::rgba(0x202228ff))
                    .cursor_pointer()
                    .when_some(preview, |view, path| {
                        view.child(img(path).size_full().object_fit(ObjectFit::Contain))
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size_12()
                                    .rounded_full()
                                    .bg(gpui_kit::rgba(0x00000099))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        svg()
                                            .path(gpui_kit::assets::IconName::Play.path())
                                            .size_6()
                                            .text_color(gpui_kit::rgb(0xffffff)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_2()
                            .right_2()
                            .px_2()
                            .rounded_md()
                            .bg(gpui_kit::rgba(0x00000099))
                            .text_xs()
                            .text_color(gpui_kit::rgb(0xffffff))
                            .child(format!(
                                "{}:{:02}",
                                reference.duration / 60,
                                reference.duration % 60
                            )),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_video(video_key.clone(), reference.clone(), cx);
                    }))
                    .context_menu(self.message_menu(message, cx, read_only, Some(index)))
                    .into_any_element()
            }
            MessageElement::Voice(reference) => {
                let key = format!("voice:{key}");
                let player = self.voice_player.read(cx);
                let active = player.key.as_ref() == Some(&key);
                let loading = active && player.loading;
                let playing = active && player.playing;
                let failed = active && player.error.is_some();
                let position = if active {
                    player.position.as_secs_f32()
                } else {
                    0.
                };
                let duration = if active && !player.duration.is_zero() {
                    player.duration.as_secs_f32()
                } else {
                    reference.duration as f32
                };
                let progress = if duration > 0. {
                    (position / duration).clamp(0., 1.)
                } else {
                    0.
                };
                let reference = reference.clone();
                div()
                    .id(SharedString::from(key.clone()))
                    .debug_selector({
                        let key = key.clone();
                        move || key.clone()
                    })
                    .cursor_pointer()
                    .w(px(180.))
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(if loading {
                                Spinner::new().into_any_element()
                            } else {
                                svg()
                                    .path(
                                        if failed {
                                            gpui_kit::assets::IconName::RefreshCw
                                        } else if playing {
                                            gpui_kit::assets::IconName::Pause
                                        } else {
                                            gpui_kit::assets::IconName::Play
                                        }
                                        .path(),
                                    )
                                    .size_5()
                                    .into_any_element()
                            })
                            .child(div().text_sm().child(if failed {
                                t!("common.retry")
                            } else if active && position > 0. {
                                t!(
                                    "voice.progress",
                                    position = position as u32,
                                    duration = duration.ceil() as u32
                                )
                            } else {
                                t!("voice.duration", duration = reference.duration)
                            })),
                    )
                    .child(
                        div()
                            .h(px(2.))
                            .w_full()
                            .rounded_full()
                            .bg(gpui_kit::rgba(0x80808040))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(progress))
                                    .rounded_full()
                                    .bg(cx.theme().primary),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let download = this
                            .voice_player
                            .update(cx, |player, cx| player.toggle(key.clone(), cx));
                        if download
                            && !this.command(Command::Voice {
                                key: key.clone(),
                                reference: reference.clone(),
                            })
                        {
                            this.voice_player.update(cx, |player, cx| {
                                player.complete(
                                    key.clone(),
                                    Err(t!("error.queue").into_owned()),
                                    cx,
                                )
                            });
                        }
                    }))
                    .context_menu(self.message_menu(message, cx, read_only, Some(index)))
                    .into_any_element()
            }
            MessageElement::File(reference) => {
                let reference = reference.clone();
                Button::new(SharedString::from(format!("media:{key}")))
                    .ghost()
                    .label(t!("chat.file", name = reference.name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.command(Command::OpenMedia(reference.clone()));
                        cx.notify();
                    }))
                    .into_any_element()
            }
            MessageElement::Forward(reference) => {
                let reference = reference.clone();
                let is_group = matches!(message.conversation, Conversation::Group { .. });
                let lines = self
                    .forward_cache
                    .get(&key)
                    .map(|rows| {
                        rows.iter()
                            .take(3)
                            .map(|message| {
                                format!(
                                    "{}: {}",
                                    self.model.sender_name(message),
                                    summary(&message.elements)
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_else(|| ferrsphen::viewer::forward_preview(&reference));
                let footer = self
                    .forward_cache
                    .get(&key)
                    .filter(|rows| !rows.is_empty())
                    .map(|rows| t!("forward.count", count = rows.len()))
                    .unwrap_or_else(|| t!("forward.view"));
                Button::new(SharedString::from(format!("forward:{key}")))
                    .ghost()
                    .h_auto()
                    .p_0()
                    .child(
                        div()
                            .debug_selector({
                                let key = key.clone();
                                move || format!("forward-card:{key}")
                            })
                            .w_56()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .p_2()
                            .rounded_lg()
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t!("forward.title")),
                            )
                            .children(lines.into_iter().map(|text| {
                                div()
                                    .w_full()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(text)
                            }))
                            .child(
                                div()
                                    .w_full()
                                    .mt(px(2.))
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(footer),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_forward(key.clone(), reference.clone(), is_group, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
            MessageElement::LightApp(json) => {
                let json = json.clone();
                let title = serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .and_then(|value| value["prompt"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| t!("common.card").into_owned());
                Button::new(SharedString::from(format!("card:{key}")))
                    .ghost()
                    .label(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.details = Some(
                            serde_json::from_str::<serde_json::Value>(&json)
                                .and_then(|value| serde_json::to_string_pretty(&value))
                                .unwrap_or_else(|_| json.clone()),
                        );
                        cx.notify();
                    }))
                    .into_any_element()
            }
            MessageElement::GroupFile(file) => {
                let file = file.clone();
                let group = match message.conversation {
                    Conversation::Group { uin } => uin,
                    _ => 0,
                };
                Button::new(SharedString::from(format!("group-file:{key}")))
                    .ghost()
                    .label(t!("chat.group_file", name = file.name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.command(Command::GroupFile {
                            group,
                            file: file.file_id.clone(),
                        });
                        cx.notify();
                    }))
                    .into_any_element()
            }
            MessageElement::Opaque(value) => {
                if let Some(face) = crate::expressions::Face::from_element(element) {
                    return self.face_image(&face);
                }
                let text = format!("字段 {}\n{}", value.field, hex::encode(&value.payload));
                Button::new(SharedString::from(format!("opaque:{key}")))
                    .ghost()
                    .label(t!("common.other_message"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.details = Some(text.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            }
        }
    }
    fn message_menu(
        &self,
        message: &Message,
        cx: &Context<Self>,
        read_only: bool,
        image_index: Option<usize>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + use<> {
        let can_react = self.can_react(message, read_only);
        let message = message.clone();
        let owner = cx.entity().downgrade();
        let recalled = self.model.is_recalled(&message);
        let has_image = message
            .elements
            .iter()
            .any(|element| matches!(element, MessageElement::Image(_)));
        let path = message
            .elements
            .iter()
            .enumerate()
            .find_map(|(index, element)| {
                if matches!(element, MessageElement::Image(_))
                    && image_index.is_none_or(|selected| selected == index)
                {
                    self.assets.get(&image_key(&message, index)).cloned()
                } else {
                    None
                }
            });
        move |menu, window, cx| {
            let recallable = !read_only
                && !recalled
                && ferrsphen::model::can_recall(&message, chrono::Utc::now().timestamp());
            if recallable {
                let source = owner.clone();
                let message = message.clone();
                let milliseconds = (message.time.saturating_add(120).saturating_mul(1000)
                    - chrono::Utc::now().timestamp_millis())
                .max(1) as u64;
                cx.spawn_in(window, async move |menu, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(milliseconds))
                        .await;
                    if let Ok(builder) = source.update(cx, |source, cx| {
                        source.message_menu(&message, cx, read_only, image_index)
                    }) {
                        let _ = menu
                            .update_in(cx, |menu, window, cx| menu.rebuild(window, cx, builder));
                    }
                })
                .detach();
            }
            let reply = message.clone();
            let recall = message.clone();
            let copied = message.clone();
            let copy_owner = owner.clone();
            let reply_owner = owner.clone();
            let recall_owner = owner.clone();
            let menu = if can_react {
                if let Some(entity) = owner.upgrade() {
                    cx.observe(&entity, |_, _, cx| cx.notify()).detach();
                }
                let source = owner.clone();
                let requested = owner.clone();
                cx.defer(move |cx| {
                    let _ = requested.update(cx, |this, cx| {
                        for id in REACTION_CHOICES {
                            this.request_face(&crate::expressions::Face::new(id));
                        }
                        cx.notify();
                    });
                });
                let target = message.clone();
                menu.min_w(px(284.)).max_w(px(284.)).item(
                    PopupMenuItem::element(move |_, cx| {
                        let Some(entity) = source.upgrade() else {
                            return div().into_any_element();
                        };
                        let this = entity.read(cx);
                        let own = this.model.profile.as_ref().map_or("", |p| p.uid.as_str());
                        let choices = REACTION_CHOICES.into_iter().map(|id| {
                                let face = crate::expressions::Face::new(id);
                                let code = id.to_string();
                                let selected = this.reaction_keys(&target).iter().any(|(key, state)| key.2 == code && state.actors.contains(own));
                                let pending = matches!(target.conversation, Conversation::Group { uin } if this.pending_reactions.contains(&(uin, target.sequence, code.clone())));
                                let source = source.clone();
                                let target = target.clone();
                                Button::new(SharedString::from(format!("reaction-choice:{id}")))
                                    .debug_selector(move || format!("reaction-choice:{id}"))
                                    .ghost()
                                    .size(px(40.))
                                    .rounded_xl()
                                    .when(selected, |button| button.bg(cx.theme().accent))
                                    .disabled(this.status == "离线" || pending)
                                    .tooltip(if selected { t!("reaction.remove_face", name = &face.name) } else { face.name.clone().into() })
                                    .accessibility_label(face.name.clone())
                                    .child(this.face_image(&face))
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        let _ = source.update(cx, |this, cx| this.toggle_reaction(&target, id.to_string(), cx));
                                        window.dispatch_action(Box::new(gpui_kit::base::actions::Cancel), cx);
                                    })
                        });
                        div()
                            .debug_selector(|| "reaction-panel".into())
                            .w(px(260.))
                            .py_2()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .bg(cx.theme().popover)
                            .rounded_xl()
                            .children(choices)
                            .into_any_element()
                    }).disabled(true),
                ).separator()
            } else {
                menu
            };
            menu.item(
                PopupMenuItem::element(|_, _| {
                    div()
                        .debug_selector(|| "message-menu-reply".into())
                        .child(t!("common.reply"))
                })
                .disabled(recalled || read_only)
                .on_click(move |_, window, cx| {
                    let _ = reply_owner.update(cx, |this, cx| {
                        this.reply = Some(reply.clone());
                        this.request_assets();
                        this.composer
                            .update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    });
                }),
            )
            .item(
                PopupMenuItem::new(t!("common.copy")).on_click(move |_, _, cx| {
                    let _ = copy_owner.update(cx, |this, cx| this.copy_message(copied.clone(), cx));
                }),
            )
            .item(
                PopupMenuItem::element(move |_, cx| {
                    div()
                        .text_color(cx.theme().foreground.opacity(if recallable {
                            1.
                        } else {
                            0.4
                        }))
                        .child(t!("common.recall"))
                })
                .disabled(!recallable)
                .on_click(move |_, _, cx| {
                    let _ = recall_owner.update(cx, |this, cx| {
                        this.command(Command::Recall(recall.clone()));
                        cx.notify();
                    });
                }),
            )
            .when(has_image && !recalled, |menu| {
                let path = path.clone();
                menu.item(
                    PopupMenuItem::new(t!("common.local_file"))
                        .disabled(path.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(path) = &path {
                                cx.reveal_path(path);
                            }
                        }),
                )
            })
        }
    }
    fn stack_expanded(&self, messages: &[Message]) -> bool {
        messages
            .iter()
            .any(|message| self.expanded_images.contains(&message_key(message)))
    }
    fn change_stack(&mut self, key: &str, expanded: bool, cx: &mut Context<Self>) {
        if expanded {
            self.expanded_images.insert(key.to_owned());
        } else {
            self.expanded_images.remove(key);
        }
        let mut sources = Vec::new();
        for rows in self
            .model
            .selected
            .as_ref()
            .and_then(|key| self.model.messages.get(key))
            .map(Vec::as_slice)
            .into_iter()
            .chain(self.forward.as_deref())
        {
            for group in message_groups(rows, &self.model.recalled) {
                if group.len() > 1 && message_key(&group[0]) == key {
                    sources.extend(group.iter().map(message_key));
                }
            }
        }
        for source in sources {
            if expanded {
                self.expanded_images.insert(source.clone());
            } else {
                self.expanded_images.remove(&source);
            }
            self.fades.remove(&source);
        }
        self.request_assets();
        cx.notify();
    }
    fn image_stack(
        &self,
        messages: &[Message],
        cx: &mut Context<Self>,
        read_only: bool,
    ) -> AnyElement {
        use ferrsphen::card_preview::{self, ACTION_HEIGHT};
        let key = message_key(&messages[0]);
        let expanded = self.stack_expanded(messages);
        let separate_messages = messages.len() > 1;
        let images = messages
            .iter()
            .flat_map(|message| {
                message.elements.iter().enumerate().filter_map(
                    move |(index, element)| match element {
                        MessageElement::Image(reference) => Some((message, index, reference)),
                        _ => None,
                    },
                )
            })
            .collect::<Vec<_>>();
        let sizes = images
            .iter()
            .map(|(_, _, reference)| card_preview::fitted(reference.width, reference.height))
            .collect::<Vec<_>>();
        let collapsed = card_preview::collapsed(&sizes);
        let visible_cards = if images.len() <= 5 { images.len() } else { 3 };
        let reaction_cells = images
            .iter()
            .map(|(message, _, _)| {
                if separate_messages && self.can_react(message, read_only) {
                    self.reaction_keys(message).len()
                } else {
                    0
                }
            })
            .collect::<Vec<_>>();
        let minimum_width = if reaction_cells.iter().any(|count| *count != 0) {
            180.
        } else if separate_messages {
            112.
        } else {
            0.
        };
        let expanded_width = sizes
            .iter()
            .map(|size| size.width)
            .fold(minimum_width, f32::max);
        let action_height = if separate_messages { ACTION_HEIGHT } else { 0. };
        let columns = ((expanded_width + 4.) / 64.).floor().max(1.) as usize;
        let reaction_heights = reaction_cells
            .iter()
            .map(|count| count.div_ceil(columns) as f32 * 32.)
            .collect::<Vec<_>>();
        let expanded_height = sizes
            .iter()
            .zip(&reaction_heights)
            .map(|(size, reactions)| size.height + action_height + reactions + 8.)
            .sum::<f32>();
        let collapsed_width = collapsed.width;
        let collapsed_height = collapsed.height;
        let mut top = 0.;
        let cards = images
            .iter()
            .enumerate()
            .map(|(order, (message, index, reference))| {
                let size = sizes[order];
                let action_height = action_height + reaction_heights[order];
                let (collapsed_x, collapsed_y) = collapsed.centers[order.min(visible_cards - 1)];
                let collapsed_size = collapsed.sizes[order.min(visible_cards - 1)];
                let collapsed_scale = if order < visible_cards {
                    collapsed_size.width / size.width
                } else {
                    1.
                };
                let expanded_top = top;
                top += size.height + action_height + 8.;
                let source_key = image_key(message, *index);
                let path = self.assets.get(&source_key).cloned();
                let sheet = self
                    .assets
                    .get(&card_key(
                        &source_key,
                        order,
                        size.width.round() as u32,
                        size.height.round() as u32,
                    ))
                    .cloned();
                let cache = self.card_textures.clone();
                let reference = (*reference).clone();
                let stack_key = key.clone();
                let expand_key = key.clone();
                let menu = self.message_menu(message, cx, read_only, Some(*index));
                let trigger = div()
                    .id(SharedString::from(format!("stack-card:{key}:{order}")))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("stack-card:{key}:{order}")
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !expanded {
                            this.change_stack(&stack_key, true, cx);
                        } else {
                            this.open_image(source_key.clone(), reference.clone(), window, cx);
                        }
                        cx.notify();
                    }))
                    .when(separate_messages && !expanded, |trigger| {
                        trigger.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _, _, cx| {
                                this.change_stack(&expand_key, true, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                    });
                let actions = (separate_messages && expanded).then(|| {
                    div()
                        .flex()
                        .flex_col()
                        .w(px(expanded_width))
                        .when(message.is_self, |bar| bar.items_end())
                        .child(self.message_actions(message, cx, read_only, true))
                        .when(reaction_heights[order] > 0., |bar| {
                            bar.child(
                                div()
                                    .w(px(expanded_width))
                                    .child(self.reaction_row(message, read_only, cx)),
                            )
                        })
                        .into_any_element()
                });
                let hover_key = message_key(message);
                let interactive = separate_messages && expanded;
                let move_key = hover_key.clone();
                let card = div()
                    .id(SharedString::from(format!("stack-hover:{key}:{order}")))
                    .absolute()
                    .flex()
                    .flex_col()
                    .items_start()
                    .when(message.is_self, |card| card.items_end())
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if interactive {
                            this.set_message_hover(&hover_key, *hovered, cx);
                        }
                    }))
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if interactive {
                            this.set_message_hover(&move_key, true, cx);
                        }
                    }));
                let self_side = message.is_self;
                let draw_key = key.clone();
                card.with_spring(
                    SharedString::from(format!("stack-card-motion:{key}:{order}")),
                    SpringAnimation::new(SpringConfig::new(260., 32., 1.))
                        .to(if expanded { 1. } else { 0. })
                        .from(if expanded { 1. } else { 0. }),
                    move |card, progress| {
                        let progress = progress.clamp(0., 1.);
                        let angle = if sheet.is_some() {
                            card_preview::angle(order) * (1. - progress)
                        } else {
                            0.
                        };
                        let scale = collapsed_scale + (1. - collapsed_scale) * progress;
                        let displayed_size = card_preview::CardSize {
                            width: size.width * scale,
                            height: size.height * scale,
                        };
                        let rotated = card_preview::bounds(displayed_size, angle);
                        let expanded_x = if self_side {
                            expanded_width - size.width / 2.
                        } else {
                            size.width / 2.
                        };
                        let expanded_y = expanded_top + size.height / 2.;
                        let left = collapsed_x + (expanded_x - collapsed_x) * progress
                            - rotated.width / 2.;
                        let top = collapsed_y + (expanded_y - collapsed_y) * progress
                            - rotated.height / 2.;
                        let visual = if let Some(sheet) = sheet.filter(|_| angle.abs() > 0.0001) {
                            card_frame(sheet, card_preview::frame(progress), cache, scale)
                        } else if let Some(path) = path {
                            img(path)
                                .id(SharedString::from(format!("stack-gif:{draw_key}:{order}")))
                                .size_full()
                                .rounded_lg()
                                .object_fit(ObjectFit::Contain)
                                .into_any_element()
                        } else {
                            div()
                                .size_full()
                                .rounded_lg()
                                .bg(gpui_kit::rgba(0x80808020))
                                .into_any_element()
                        };
                        let trigger = trigger
                            .w(px(rotated.width))
                            .h(px(rotated.height))
                            .child(visual);
                        let trigger = if separate_messages && !expanded {
                            trigger.into_any_element()
                        } else {
                            trigger.context_menu(menu).into_any_element()
                        };
                        card.left(px(left))
                            .top(px(top))
                            .w(px(rotated.width + if interactive { 16. } else { 0. }))
                            .h(px(
                                rotated.height + if interactive { action_height + 16. } else { 0. }
                            ))
                            .when(interactive, |card| card.p_2().m(px(-8.)))
                            .when(order >= visible_cards && progress <= 0.001, |card| {
                                card.invisible()
                            })
                            .when(order >= visible_cards, |card| card.opacity(progress))
                            .child(trigger)
                            .when_some(actions, |card, actions| {
                                card.child(div().h(px(action_height)).child(actions))
                            })
                    },
                )
                .into_any_element()
            })
            .collect::<Vec<_>>();
        let collapse_key = key.clone();
        div()
            .debug_selector(|| format!("image-stack:{key}"))
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(
                div()
                    .relative()
                    .children(cards.into_iter().rev())
                    .with_spring(
                        SharedString::from(format!("stack-geometry:{key}")),
                        SpringAnimation::new(SpringConfig::new(260., 32., 1.))
                            .to(if expanded { 1. } else { 0. })
                            .from(if expanded { 1. } else { 0. }),
                        move |stack, progress| {
                            let progress = progress.clamp(0., 1.);
                            stack
                                .w(px(
                                    collapsed_width + (expanded_width - collapsed_width) * progress
                                ))
                                .h(px(collapsed_height
                                    + (expanded_height - collapsed_height) * progress))
                        },
                    ),
            )
            .when(expanded, |view| {
                view.child(
                    Button::new(SharedString::from(format!("collapse:{key}")))
                        .ghost()
                        .xsmall()
                        .icon(gpui_kit::assets::IconName::ChevronUp)
                        .tooltip(t!("image.collapse"))
                        .accessibility_label(t!("image.collapse"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_stack(&collapse_key, false, cx);
                        })),
                )
            })
            .into_any_element()
    }
    fn can_react(&self, message: &Message, read_only: bool) -> bool {
        !read_only
            && !self.model.is_recalled(message)
            && message.sequence != 0
            && matches!(message.conversation, Conversation::Group { uin } if uin != 0)
    }
    fn reaction_keys(
        &self,
        message: &Message,
    ) -> Vec<(crate::activity::ReactionKey, crate::activity::ReactionState)> {
        let Conversation::Group { uin } = message.conversation else {
            return vec![];
        };
        let mut rows = self
            .model
            .reactions
            .iter()
            .filter(|((group, sequence, _), state)| {
                *group == uin && *sequence == message.sequence && state.count > 0
            })
            .map(|(key, state)| (key.clone(), state.clone()))
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.0.2.cmp(&b.0.2));
        rows
    }
    fn toggle_reaction(&mut self, message: &Message, code: String, cx: &mut Context<Self>) {
        if !self.can_react(message, false) || self.status == "离线" {
            return;
        }
        let Conversation::Group { uin } = message.conversation else {
            return;
        };
        let key = (uin, message.sequence, code.clone());
        if self.pending_reactions.contains(&key) {
            return;
        }
        let own = self
            .model
            .profile
            .as_ref()
            .map_or("", |profile| profile.uid.as_str());
        let added = !self
            .model
            .reactions
            .get(&key)
            .is_some_and(|state| state.actors.contains(own));
        if self.command(Command::Reaction {
            group: uin,
            sequence: message.sequence,
            code,
            added,
        }) {
            self.pending_reactions.insert(key);
        }
        cx.notify();
    }
    fn reaction_symbol(&self, code: &str) -> AnyElement {
        if let Ok(id) = code.parse::<u32>() {
            if id < 4096 {
                return self.face_image(&crate::expressions::Face::new(id));
            }
            if let Some(character) = char::from_u32(id) {
                return div()
                    .text_sm()
                    .child(character.to_string())
                    .into_any_element();
            }
        }
        div().text_sm().child(code.to_owned()).into_any_element()
    }
    fn reaction_row(
        &self,
        message: &Message,
        read_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.can_react(message, read_only) {
            return div().into_any_element();
        }
        let own = self
            .model
            .profile
            .as_ref()
            .map_or("", |profile| profile.uid.as_str());
        let rows = self.reaction_keys(message).into_iter().map(|(key, state)| {
            let code = key.2.clone();
            let message = message.clone();
            let mine = state.actors.contains(own);
            let selector = format!("reaction:{}:{}:{}", key.0, key.1, code);
            Button::new(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .ghost()
                .xsmall()
                .w(px(60.))
                .h(px(28.))
                .px_1()
                .when(mine, |button| button.bg(cx.theme().accent))
                .disabled(self.status == "离线" || self.pending_reactions.contains(&key))
                .tooltip(format!(
                    "{} · {}",
                    if mine {
                        t!("reaction.remove")
                    } else {
                        t!("reaction.add")
                    },
                    state.count
                ))
                .accessibility_label(t!("reaction.label", code = code, count = state.count))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(self.reaction_symbol(&code))
                        .child(div().text_xs().child(if state.count > 99 {
                            "99+".into()
                        } else {
                            state.count.to_string()
                        })),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_reaction(&message, code.clone(), cx)
                }))
        });
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(rows)
            .into_any_element()
    }
    fn message_actions(
        &self,
        message: &Message,
        cx: &mut Context<Self>,
        read_only: bool,
        timestamp: bool,
    ) -> AnyElement {
        let key = message_key(message);
        let recalled = self.model.is_recalled(message);
        let fade = self.fades.get(&key);
        let (from, to, generation) = fade
            .map(|fade| (fade.from, fade.to, fade.generation))
            .unwrap_or((0., 0., 0));
        let reply = message.clone();
        let copied = message.clone();
        let actions = div()
            .flex()
            .gap_1()
            .child(
                Button::new(SharedString::from(format!("reply:{key}")))
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Reply)
                    .disabled(recalled || read_only)
                    .tooltip(t!("common.reply"))
                    .accessibility_label(t!("common.reply"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reply = Some(reply.clone());
                        this.request_assets();
                        this.composer
                            .update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    })),
            )
            .child(
                Button::new(SharedString::from(format!("copy:{key}")))
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Copy)
                    .tooltip(t!("common.copy"))
                    .accessibility_label(t!("common.copy"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.copy_message(copied.clone(), cx)),
                    ),
            )
            .with_animation(
                SharedString::from(format!("actions:{key}:{generation}")),
                Animation::new(Duration::from_millis(80)),
                move |row, progress| {
                    let opacity = from + (to - from) * progress;
                    row.opacity(opacity)
                        .when(opacity <= 0., |row| row.invisible())
                },
            );
        let action_hover = key.clone();
        let action_move = key.clone();
        div()
            .id(SharedString::from(format!("action-hover:{key}")))
            .debug_selector(move || format!("message-actions:{key}"))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if timestamp {
                    this.set_message_hover(&action_hover, *hovered, cx);
                }
            }))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if timestamp {
                    this.set_message_hover(&action_move, true, cx);
                }
            }))
            .flex()
            .items_center()
            .gap_2()
            .child(actions)
            .when(timestamp, |bar| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(clock_time(message.time))
                        .with_animation(
                            SharedString::from(format!(
                                "image-time:{}:{generation}",
                                message_key(message)
                            )),
                            Animation::new(Duration::from_millis(80)),
                            move |label, progress| label.opacity(from + (to - from) * progress),
                        ),
                )
            })
            .into_any_element()
    }
    fn set_message_hover(&mut self, key: &str, hovered: bool, cx: &mut Context<Self>) {
        let to = if hovered { 1. } else { 0. };
        let old = self.fades.get(key);
        if old.is_some_and(|fade| fade.to == to) {
            return;
        }
        let from = old.map_or(0., Fade::value);
        let generation = old.map_or(1, |fade| fade.generation + 1);
        self.fades.insert(
            key.to_owned(),
            Fade {
                from,
                to,
                started: Instant::now(),
                generation,
            },
        );
        cx.notify();
    }
    fn message_row(
        &self,
        messages: &[Message],
        window: &Window,
        cx: &mut Context<Self>,
        read_only: bool,
    ) -> AnyElement {
        let message = &messages[0];
        let separate_messages = messages.len() > 1;
        let key = message_key(message);
        let recalled = self.model.is_recalled(message);
        let image_only = image_only(message);
        let image_count = messages
            .iter()
            .flat_map(|message| &message.elements)
            .filter(|element| matches!(element, MessageElement::Image(_)))
            .count();
        let mut content = Vec::new();
        if recalled {
            content.push(
                div()
                    .debug_selector({
                        let key = key.clone();
                        move || format!("recall-label:{key}")
                    })
                    .text_size(window.rem_size() * 0.875)
                    .line_height(px(20.))
                    .font_family(cx.theme().font_family.clone())
                    .min_w_0()
                    .truncate()
                    .child(t!("common.recalled"))
                    .into_any_element(),
            );
        } else {
            let mut stacked = false;
            for (index, element) in message.elements.iter().enumerate() {
                if image_only && image_count > 1 && matches!(element, MessageElement::Image(_)) {
                    if !stacked {
                        content.push(self.image_stack(messages, cx, read_only));
                        stacked = true;
                    }
                } else {
                    content.push(self.element(message, index, element, cx, read_only));
                }
            }
        }
        let body = div()
            .debug_selector(|| format!("message-bubble:{key}"))
            .flex()
            .flex_col()
            .gap_2()
            .rounded_lg()
            .when(image_only || recalled, |body| body.overflow_hidden())
            .when(!image_only || recalled, |body| {
                body.px_3().py_2().bg(if message.is_self {
                    cx.theme().accent
                } else {
                    cx.theme().muted
                })
            })
            .children(content);
        let tween = (!read_only).then(|| self.recall_tweens.get(&key)).flatten();
        let progress = tween.map_or(1., |tween| {
            tween
                .state(cx.background_executor().now())
                .position
                .clamp(0., 1.)
        });
        let body = if recalled {
            let target = recalled_size(window, cx);
            let original = tween.map_or(target, |tween| tween.original);
            body.w(original.width + (target.width - original.width) * progress)
                .h(original.height + (target.height - original.height) * progress)
                .into_any_element()
        } else {
            body.into_any_element()
        };
        // The menu belongs to the painted body; proximity padding is outside this element.
        let body = if image_only && !recalled {
            body
        } else {
            div()
                .child(body)
                .context_menu(self.message_menu(message, cx, read_only, None))
                .into_any_element()
        };
        let measured = self.bubble_sizes.clone();
        let visible = self.visible_bubbles.clone();
        let scroll = self.scroll.clone();
        let measured_key = key.clone();
        let body = div()
            .child(body)
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    if !recalled {
                        measured
                            .borrow_mut()
                            .insert(measured_key.clone(), bounds.size);
                    }
                    if !read_only && bounds.intersects(&scroll.bounds()) {
                        visible
                            .borrow_mut()
                            .insert(measured_key.clone(), bounds.size);
                    }
                }
            })
            .opacity(if recalled { 1. - 0.35 * progress } else { 1. });
        let fade = self.fades.get(&key);
        let (from, to, generation) = fade
            .map(|fade| (fade.from, fade.to, fade.generation))
            .unwrap_or((0., 0., 0));
        let hover_key = key.clone();
        let actions = self.message_actions(message, cx, read_only, false);
        div()
            .id(SharedString::from(key.clone()))
            .flex()
            .w_full()
            .gap_3()
            .items_start()
            .when(message.is_self, |row| row.flex_row_reverse())
            .child(self.participant_avatar(
                self.model.sender_uin(message),
                &message.sender_uid,
                &self.model.sender_name(message),
                &message.conversation,
                read_only,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .max_w(rems(36.))
                    .when(self.phone, |col| col.flex_1().min_w_0())
                    .when(message.is_self, |col| col.items_end())
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .when(message.is_self, |header| header.flex_row_reverse())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.model.sender_name(message))
                            .when(!separate_messages, |row| {
                                row.child(div().child(clock_time(message.time)).with_animation(
                                    SharedString::from(format!("time:{key}:{generation}")),
                                    Animation::new(Duration::from_millis(80)),
                                    move |label, progress| {
                                        label.opacity(from + (to - from) * progress)
                                    },
                                ))
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("proximity:{key}")))
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .p_2()
                            .m(px(-8.))
                            .when(message.is_self, |col| col.items_end())
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if !separate_messages {
                                    this.set_message_hover(&hover_key, *hovered, cx);
                                }
                            }))
                            .child(body)
                            .when(!separate_messages, |col| {
                                col.child(self.reaction_row(message, read_only, cx))
                                    .child(actions)
                            }),
                    ),
            )
            .into_any_element()
    }
    fn lazy_message_rows(
        &self,
        messages: &[Message],
        window: &Window,
        cx: &mut Context<Self>,
        read_only: bool,
    ) -> Vec<AnyElement> {
        let groups = message_groups(messages, &self.model.recalled);
        let scroll = if read_only {
            &self.forward_scroll
        } else {
            &self.scroll
        };
        let viewport = scroll.bounds();
        let overscan = px(600.);
        let visible = Bounds::new(
            point(viewport.left(), viewport.top() - overscan),
            size(viewport.size.width, viewport.size.height + overscan * 2.),
        );
        let initialized = viewport.size.height > px(0.);
        let anchor = if !read_only
            && self.scroll_anchor.is_none()
            && self.send_scroll.is_none()
            && initialized
        {
            self.capture_timeline_anchor().map(|mut anchor| {
                anchor.follow_tail = (scroll.offset().y + scroll.max_offset().y).abs() < px(80.);
                anchor
            })
        } else {
            None
        };
        groups
            .iter()
            .enumerate()
            .map(|(index, messages)| {
                let key = format!(
                    "{}:{}",
                    if read_only { "forward" } else { "chat" },
                    message_key(&messages[0])
                );
                let in_view = scroll
                    .bounds_for_item(index)
                    .map(|mut bounds| {
                        bounds.origin.y += scroll.offset().y;
                        bounds.intersects(&visible)
                    })
                    .unwrap_or_else(|| {
                        if read_only {
                            index < 20
                        } else {
                            index + 20 >= groups.len()
                        }
                    });
                if in_view
                    || !initialized
                        && (if read_only {
                            index < 20
                        } else {
                            index + 20 >= groups.len()
                        })
                {
                    let cache = self.lazy_heights.clone();
                    let owner = cx.entity().downgrade();
                    let anchor = anchor.clone();
                    div()
                        .w_full()
                        .flex_shrink_0()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .w_full()
                                .child(self.message_row(messages, window, cx, read_only))
                                .children(self.poke_rows(messages, read_only, cx)),
                        )
                        .on_children_prepainted(move |bounds, _, cx| {
                            if let Some(bounds) = bounds.first() {
                                let old = cache
                                    .borrow_mut()
                                    .insert(key.clone(), bounds.size.height)
                                    .unwrap_or(px(100.));
                                if (old - bounds.size.height).abs() > px(0.5)
                                    && let Some(anchor) = anchor.clone()
                                {
                                    let owner = owner.clone();
                                    cx.defer(move |cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            if this.scroll_anchor.is_none()
                                                && this.send_scroll.is_none()
                                                && this.reply_jump.is_none()
                                                && this.model.selected.as_ref()
                                                    == Some(&anchor.conversation)
                                            {
                                                this.scroll_anchor = Some(anchor);
                                                cx.notify();
                                            }
                                        });
                                    });
                                }
                            }
                        })
                        .into_any_element()
                } else {
                    let height = self
                        .lazy_heights
                        .borrow()
                        .get(&key)
                        .copied()
                        .unwrap_or(px(100.));
                    let owner = cx.entity().downgrade();
                    let scroll = scroll.clone();
                    div()
                        .w_full()
                        .h(height)
                        .flex_shrink_0()
                        .child(
                            canvas(
                                move |bounds, _, cx| {
                                    let viewport = scroll.bounds();
                                    let visible = Bounds::new(
                                        point(viewport.left(), viewport.top() - overscan),
                                        size(
                                            viewport.size.width,
                                            viewport.size.height + overscan * 2.,
                                        ),
                                    );
                                    if bounds.intersects(&visible) {
                                        cx.defer(move |cx| {
                                            let _ = owner.update(cx, |_, cx| cx.notify());
                                        });
                                    }
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .relative()
                        .into_any_element()
                }
            })
            .collect()
    }

    fn conversation(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat) = self.model.selected_chat() else {
            return div().flex_1().into_any_element();
        };
        let key = conversation_key(&chat.conversation);
        let history = self.model.history.get(&key);
        let loading = history.is_some_and(|history| history.loading);
        let error = history.and_then(|history| history.error.clone());
        let mut rows = self.lazy_message_rows(
            self.model
                .messages
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            window,
            cx,
            false,
        );
        if rows.is_empty()
            && let Some(notices) = self.poke_notices.get(&key)
        {
            let leading = notices
                .iter()
                .filter(|(after, _)| after.is_none())
                .map(|(_, text)| {
                    div()
                        .debug_selector(|| "poke-notice".into())
                        .w_full()
                        .py_2()
                        .text_center()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(text.clone())
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            rows.splice(0..0, leading);
        }
        let pending = self
            .model
            .pending
            .iter()
            .filter(|(id, value)| value.0 == key && !self.model.optimistic.contains_key(id))
            .map(|(id, (_, text))| {
                div()
                    .id(SharedString::from(format!("pending:{id}")))
                    .flex()
                    .justify_end()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("chat.sending", text = text))
            })
            .collect::<Vec<_>>();
        let failed = self
            .model
            .failures
            .iter()
            .filter(|(_, value)| value.0 == key)
            .map(|(id, (_, text, error))| {
                let id = *id;
                let text = text.clone();
                let retry_key = key.clone();
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(t!("chat.failed", error = error))
                    .child(
                        Button::new(SharedString::from(format!("retry:{id}")))
                            .ghost()
                            .small()
                            .label(t!("common.retry"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.retry_send(id, &retry_key, &text, cx);
                            })),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .id("chat-drop-target")
            .debug_selector(|| "chat-drop-target".into())
            .on_drag_move::<ExternalPaths>(|event, window, cx| {
                // GPUI suppresses drop hit testing after keyboard input until mouse modality resumes.
                if window.last_input_was_keyboard() {
                    let event = event.event.clone();
                    window.defer(cx, move |window, cx| {
                        if cx.has_active_drag() {
                            window.dispatch_event(gpui_kit::PlatformInput::MouseMove(event), cx);
                        }
                    });
                }
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.receive_paths(paths.paths(), window, cx)
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(if self.phone { 52. } else { 48. }))
                    .flex_shrink_0()
                    .gap_2()
                    .when(self.phone, |header| header.px_2())
                    .when(!self.phone, |header| header.px_5())
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .when(self.phone, |header| {
                        header.child(
                            Button::new("back-to-chats")
                                .debug_selector(|| "back-to-chats".into())
                                .ghost()
                                .size(px(44.))
                                .icon(gpui_kit::assets::IconName::ChevronLeft)
                                .accessibility_label(t!("chat.back"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_chat_list(window, cx)
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(chat.title.clone()),
                    ),
            )
            .when_some(error, |view, error| {
                view.child(
                    div()
                        .px_5()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(TimelineViewport::new(
                        div()
                            .id(SharedString::from(format!("timeline:{key}")))
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .when(self.phone, |timeline| timeline.px_3())
                            .when(!self.phone, |timeline| timeline.px_5())
                            .py_4()
                            .gap_5()
                            .children(rows)
                            .children(pending)
                            .children(failed),
                        px(self.bounce.position),
                        {
                            let owner = cx.entity();
                            move |event, window, cx| {
                                owner.update(cx, |this, cx| this.timeline_wheel(event, window, cx));
                            }
                        },
                    ))
                    .when(loading, |view| {
                        view.child(
                            div()
                                .id("history-loading")
                                .debug_selector(|| "history-loading".into())
                                .absolute()
                                .top_2()
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .size_8()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(cx.theme().background)
                                        .shadow_sm()
                                        .child(
                                            Spinner::new()
                                                .small()
                                                .color(cx.theme().muted_foreground),
                                        ),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .debug_selector(|| "composer".into())
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .when(self.phone, |composer| composer.p_2().gap_1())
                    .when(!self.phone, |composer| composer.p_3().gap_2())
                    .capture_action(cx.listener(
                        |this, _: &gpui_kit::component::input::Paste, window, cx| {
                            if this.composer.read(cx).focus_handle(cx).is_focused(window)
                                && this.paste_draft(window, cx)
                            {
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .capture_action(cx.listener(
                        |this, _: &gpui_kit::component::input::Copy, window, cx| {
                            if this.composer.read(cx).focus_handle(cx).is_focused(window)
                                && this.copy_draft(false, window, cx)
                            {
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .capture_action(cx.listener(
                        |this, _: &gpui_kit::component::input::Cut, window, cx| {
                            if this.composer.read(cx).focus_handle(cx).is_focused(window)
                                && this.copy_draft(true, window, cx)
                            {
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(self.expression_picker(cx))
                            .child(
                                Button::new("image-upload")
                                    .debug_selector(|| "image-upload".into())
                                    .ghost()
                                    .small()
                                    .icon(gpui_kit::assets::IconName::Image)
                                    .size(px(if self.phone { 44. } else { 32. }))
                                    .tooltip(t!("common.image"))
                                    .accessibility_label(t!("common.image"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.attach(AttachmentKind::Image, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("file-upload")
                                    .debug_selector(|| "file-upload".into())
                                    .ghost()
                                    .small()
                                    .icon(gpui_kit::assets::IconName::Paperclip)
                                    .size(px(if self.phone { 44. } else { 32. }))
                                    .tooltip(t!("common.file"))
                                    .accessibility_label(t!("common.file"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.attach(AttachmentKind::File, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("voice-upload")
                                    .debug_selector(|| "voice-upload".into())
                                    .ghost()
                                    .small()
                                    .icon(gpui_kit::assets::IconName::Mic)
                                    .size(px(if self.phone { 44. } else { 32. }))
                                    .tooltip(t!("common.voice"))
                                    .accessibility_label(t!("common.voice"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.attach(AttachmentKind::Voice, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("video-upload")
                                    .debug_selector(|| "video-upload".into())
                                    .ghost()
                                    .small()
                                    .icon(gpui_kit::assets::IconName::Clapperboard)
                                    .size(px(if self.phone { 44. } else { 32. }))
                                    .tooltip(t!("common.video"))
                                    .accessibility_label(t!("common.video"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.attach(AttachmentKind::Video, window, cx)
                                    })),
                            ),
                    )
                    .when_some(self.reply.as_ref(), |view, reply| {
                        view.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().flex_1().min_w_0().child(self.reply_card(reply, cx)))
                                .child(
                                    Button::new("cancel-reply")
                                        .ghost()
                                        .xsmall()
                                        .icon(gpui_kit::assets::IconName::X)
                                        .tooltip(t!("chat.cancel_reply"))
                                        .accessibility_label(t!("chat.cancel_reply"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reply = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .gap_2()
                            .when(self.phone, |row| row.items_end())
                            .when(!self.phone, |row| row.flex_col())
                            .child(
                                div()
                                    .debug_selector(|| "composer-editor".into())
                                    .min_w_0()
                                    .when(self.phone, |editor| editor.flex_1())
                                    .child(
                                        Textarea::new(&self.composer)
                                            .token({
                                                let images = self.compose_images.clone();
                                                let faces = self.compose_faces.clone();
                                                let assets = self.assets.clone();
                                                move |context, _, _| {
                                                    if let Some(face) =
                                                        faces.get(context.token().id().as_ref())
                                                    {
                                                        return face_image(face, &assets);
                                                    }
                                                    div()
                                                        .debug_selector(|| "composer-image".into())
                                                        .w(px(112.))
                                                        .h(px(72.))
                                                        .rounded_md()
                                                        .overflow_hidden()
                                                        .when_some(
                                                            images
                                                                .get(context.token().id().as_ref())
                                                                .cloned(),
                                                            |view, path| {
                                                                view.child(
                                                                    img(path)
                                                                        .size_full()
                                                                        .object_fit(
                                                                            ObjectFit::Contain,
                                                                        ),
                                                                )
                                                            },
                                                        )
                                                        .into_any_element()
                                                }
                                            })
                                            .appearance(false)
                                            .when(!self.phone, |input| input.h_24())
                                            .when(
                                                self.composer.read(cx).tokens().iter().any(
                                                    |span| {
                                                        self.compose_images.contains_key(
                                                            span.token().id().as_ref(),
                                                        )
                                                    },
                                                ),
                                                |input| {
                                                    input
                                                        .line_height(px(80.))
                                                        .when(!self.phone, |input| {
                                                            input.h(px(160.))
                                                        })
                                                },
                                            )
                                            .bordered(false)
                                            .aria_label(t!("chat.contents")),
                                    ),
                            )
                            .child(
                                div().flex().justify_end().child(
                                    Button::new("send")
                                        .debug_selector(|| "send-message".into())
                                        .primary()
                                        .accessibility_label(t!("common.send"))
                                        .when(self.phone, |button| {
                                            button
                                                .icon(gpui_kit::assets::IconName::ArrowUp)
                                                .size(px(44.))
                                        })
                                        .when(!self.phone, |button| button.label(t!("common.send")))
                                        .disabled(
                                            self.composer.read(cx).value().trim().is_empty()
                                                || self.status == "离线",
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.send(window, cx)
                                            }),
                                        ),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn info_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(Conversation::Group { uin }) =
            self.model.selected_chat().map(|chat| &chat.conversation)
        else {
            return div().into_any_element();
        };
        let group = *uin;
        let members = self.model.members.get(&group);
        let rows = members
            .map(|members| {
                members
                    .iter()
                    .map(|member| self.member_row(member, cx))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let announcements = match self.model.announcements.get(&group) {
            Some(Ok(items)) => div()
                .flex()
                .flex_col()
                .gap_3()
                .when(items.is_empty(), |view| {
                    view.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("group.no_announcements")),
                    )
                })
                .children(items.iter().map(|notice| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .when(!notice.title.is_empty(), |view| {
                            view.child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(notice.title.clone()),
                            )
                        })
                        .child(div().text_sm().child(notice.text.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    chrono::DateTime::from_timestamp(notice.published_at, 0)
                                        .map(|date| {
                                            date.with_timezone(&chrono::Local)
                                                .format("%m-%d %H:%M")
                                                .to_string()
                                        })
                                        .unwrap_or_default(),
                                ),
                        )
                }))
                .into_any_element(),
            Some(Err(_)) => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t!("common.load_failed"))
                .into_any_element(),
            None => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t!("common.loading"))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .debug_selector(|| "group-sidebar".into())
            .w_64()
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(t!("group.announcements")),
                            )
                            .child(
                                Button::new("refresh-announcements")
                                    .ghost()
                                    .xsmall()
                                    .icon(gpui_kit::assets::IconName::RefreshCw)
                                    .accessibility_label(t!("group.refresh_announcements"))
                                    .tooltip(t!("common.refresh"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.model.announcements.remove(&group);
                                        this.command(Command::Announcements(group));
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("announcements")
                            .max_h(rems(18.))
                            .overflow_y_scroll()
                            .child(announcements),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .child(t!("group.members", count = members.map_or(0, Vec::len))),
            )
            .child(
                div()
                    .id("member-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .children(rows),
            )
            .into_any_element()
    }
    fn member_row(&self, member: &GroupMember, cx: &mut Context<Self>) -> AnyElement {
        let name = ferrsphen::model::display_name(
            self.model.friends.iter().find(|friend| {
                (!member.uid.is_empty() && friend.uid == member.uid)
                    || (member.uin != 0 && friend.uin == member.uin)
            }),
            Some(member),
            &member.nickname,
            member.uin,
        );
        let conversation = self
            .model
            .selected_chat()
            .map(|chat| chat.conversation.clone());
        div()
            .flex()
            .items_center()
            .gap_2()
            .py_2()
            .child(match conversation {
                Some(conversation) => self.participant_avatar(
                    member.uin,
                    &member.uid,
                    &name,
                    &conversation,
                    false,
                    cx,
                ),
                None => self.avatar(member.uin, &name, cx),
            })
            .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
            .into_any_element()
    }

    fn open_video(
        &mut self,
        key: String,
        reference: atri_qq::media::MediaReference,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity();
        cx.defer(move |cx| {
            if let Some(handle) = owner.read(cx).video_windows.get(&key).copied()
                && handle
                    .update(cx, |_, window, _| window.activate_window())
                    .is_ok()
            {
                return;
            }
            owner.update(cx, |this, cx| {
                if !matches!(this.video_results.get(&key), Some(Ok(_))) {
                    this.video_results.remove(&key);
                    if !this.command(Command::ViewVideo {
                        key: key.clone(),
                        reference,
                    }) {
                        this.video_results
                            .insert(key.clone(), Err(t!("error.queue").into_owned()));
                    }
                }
                cx.notify();
            });
            let content_owner = owner.clone();
            let content_key = key.clone();
            let result = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(size(px(960.), px(680.)), cx)),
                    window_min_size: Some(size(px(520.), px(360.))),
                    ..ferrsphen::window_chrome::options(t!("common.video"))
                },
                cx,
                move |window, cx| {
                    cx.new(|cx| {
                        let mut view = ferrsphen::video::VideoView::new(window, cx);
                        if let Some(result) = content_owner
                            .read(cx)
                            .video_results
                            .get(&content_key)
                            .cloned()
                        {
                            view.complete_download(result, cx);
                        }
                        let observer = cx.observe(
                            &content_owner,
                            move |view: &mut ferrsphen::video::VideoView, owner, cx| {
                                if let Some(result) =
                                    owner.read(cx).video_results.get(&content_key).cloned()
                                {
                                    view.complete_download(result, cx);
                                }
                            },
                        );
                        view.watch(observer);
                        view
                    })
                },
            );
            owner.update(cx, |this, cx| {
                match result {
                    Ok((handle, _)) => {
                        this.video_windows.insert(key, handle);
                    }
                    Err(error) => {
                        tracing::error!(?error, "open video window failed");
                        this.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        });
    }

    fn open_detail_window(&mut self, kind: DetailKind, cx: &mut Context<Self>) {
        let owner = cx.entity();
        // Native window creation may synchronously render its content.
        cx.defer(move |cx| {
            let existing = match kind {
                DetailKind::Image => owner.read(cx).viewer_window,
                DetailKind::Forward => owner.read(cx).forward_window,
            };
            let focus = owner.read(cx).viewer_focus.clone();
            if let Some(handle) = existing
                && handle
                    .update(cx, |_, window, cx| {
                        window.activate_window();
                        if matches!(kind, DetailKind::Image) {
                            focus.focus(window, cx);
                        }
                    })
                    .is_ok()
            {
                return;
            }
            let dimensions = match kind {
                DetailKind::Image => size(px(960.), px(760.)),
                DetailKind::Forward => size(px(600.), px(720.)),
            };
            let title = match kind {
                DetailKind::Image => t!("common.image"),
                DetailKind::Forward => t!("forward.title"),
            };
            let content_owner = owner.clone();
            let result = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(dimensions, cx)),
                    window_min_size: Some(size(px(420.), px(360.))),
                    ..ferrsphen::window_chrome::options(title)
                },
                cx,
                move |window, cx| {
                    if matches!(kind, DetailKind::Image) {
                        focus.focus(window, cx);
                    }
                    cx.new(|cx| {
                        let observer = cx.observe(&content_owner, |_, _, cx| cx.notify());
                        DetailWindow {
                            owner: content_owner,
                            kind,
                            _observer: observer,
                        }
                    })
                },
            );
            owner.update(cx, |owner, cx| {
                match result {
                    Ok((window, _)) => match kind {
                        DetailKind::Image => owner.viewer_window = Some(window),
                        DetailKind::Forward => owner.forward_window = Some(window),
                    },
                    Err(error) => owner.error = Some(error.to_string()),
                }
                cx.notify();
            });
        });
    }
    fn open_forward(
        &mut self,
        key: String,
        reference: atri_qq::messages::ForwardReference,
        is_group: bool,
        cx: &mut Context<Self>,
    ) {
        if self.forward_key.as_ref() != Some(&key) {
            self.forward_scroll.scroll_to_top_of_item(0);
            if self.reply_jump.as_ref().is_some_and(|jump| jump.read_only) {
                self.reply_jump = None;
            }
        }
        self.forward = self.forward_cache.get(&key).cloned();
        self.forward_error = None;
        self.forward_key = Some(key.clone());
        if self.forward.is_none() {
            self.command(Command::Forward {
                key,
                reference,
                is_group,
            });
        }
        self.request_assets();
        self.open_detail_window(DetailKind::Forward, cx);
    }
    fn forward_view(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                ferrsphen::window_chrome::title_bar(cx).child(
                    div()
                        .flex_1()
                        .text_center()
                        .text_sm()
                        .child(t!("forward.title")),
                ),
            )
            .child(
                div()
                    .id("forward-messages")
                    .track_scroll(&self.forward_scroll)
                    .on_scroll_wheel(cx.listener(|this, _, window, cx| {
                        this.reply_jump = None;
                        cx.defer_in(window, |this, _, cx| {
                            this.request_assets();
                            cx.notify();
                        });
                    }))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .when_some(self.forward_error.as_ref(), |view, error| {
                        view.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(error.clone()),
                        )
                    })
                    .when(
                        self.forward.is_none() && self.forward_error.is_none(),
                        |view| {
                            view.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t!("common.loading")),
                            )
                        },
                    )
                    .children(self.lazy_message_rows(
                        self.forward.as_deref().unwrap_or_default(),
                        window,
                        cx,
                        true,
                    )),
            )
            .into_any_element()
    }
    fn open_image(
        &mut self,
        key: String,
        reference: atri_qq::media::MediaReference,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut images =
            self.model
                .selected
                .as_ref()
                .and_then(|key| self.model.messages.get(key))
                .map(|messages| {
                    messages
                        .iter()
                        .flat_map(|message| {
                            message.elements.iter().enumerate().filter_map(
                                move |(index, element)| match element {
                                    MessageElement::Image(reference) => Some(ImageItem {
                                        key: image_key(message, index),
                                        reference: reference.clone(),
                                    }),
                                    _ => None,
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
        if !images.iter().any(|image| image.key == key)
            && let Some(messages) = &self.forward
        {
            images = messages
                .iter()
                .flat_map(|message| {
                    message
                        .elements
                        .iter()
                        .enumerate()
                        .filter_map(move |(index, element)| match element {
                            MessageElement::Image(reference) => Some(ImageItem {
                                key: image_key(message, index),
                                reference: reference.clone(),
                            }),
                            _ => None,
                        })
                })
                .collect();
        }
        if !images.iter().any(|image| image.key == key) {
            images.push(ImageItem {
                key: key.clone(),
                reference,
            });
        }
        self.viewer = Viewer::new(images, &key);
        self.request_viewer_image();
        self.open_detail_window(DetailKind::Image, cx);
    }
    fn request_viewer_image(&mut self) {
        if let Some(viewer) = &self.viewer
            && !self.assets.contains_key(&viewer.current().key)
        {
            let image = viewer.current().clone();
            self.command(Command::ViewImage {
                key: image.key,
                reference: image.reference,
            });
        }
    }
    fn navigate_image(&mut self, direction: isize) {
        if let Some(viewer) = &mut self.viewer
            && viewer.navigate(direction)
        {
            self.request_viewer_image();
        }
    }
    fn close_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer = None;
        self.viewer_window = None;
        let handle = window.window_handle();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
        cx.notify();
    }
    fn image_viewer(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let viewer = self.viewer.as_ref().expect("image viewer");
        let item = viewer.current();
        let path = self.assets.get(&item.key).cloned();
        let width = f32::from(window.viewport_size().width);
        let height = (f32::from(window.viewport_size().height) - 112.).max(100.);
        let available = ((width - 96.).max(100.), (height - 32.).max(100.));
        let fit = if item.reference.width > 0 && item.reference.height > 0 {
            let factor = (available.0 / item.reference.width as f32)
                .min(available.1 / item.reference.height as f32);
            (
                item.reference.width as f32 * factor,
                item.reference.height as f32 * factor,
            )
        } else {
            available
        };
        let image_width = fit.0 * viewer.zoom;
        let image_height = fit.1 * viewer.zoom;
        let image = match path {
            Some(path) => {
                let file = path.clone();
                div()
                    .absolute()
                    .left(px((width - image_width) / 2. + viewer.pan.0))
                    .top(px((height - image_height) / 2. + viewer.pan.1))
                    .w(px(image_width))
                    .h(px(image_height))
                    .child(
                        img(path)
                            .id(SharedString::from(format!("viewer:{}", item.key)))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .context_menu(move |menu, _, _| {
                        let file = file.clone();
                        menu.item(
                            PopupMenuItem::new(t!("common.local_file"))
                                .on_click(move |_, _, cx| cx.reveal_path(&file)),
                        )
                    })
                    .into_any_element()
            }
            None => div()
                .flex()
                .size_full()
                .items_center()
                .justify_center()
                .flex_col()
                .gap_3()
                .child(if viewer.error.is_some() {
                    t!("common.load_failed")
                } else {
                    t!("common.loading")
                })
                .when(viewer.error.is_some(), |view| {
                    view.child(
                        Button::new("retry-image")
                            .ghost()
                            .label(t!("common.retry"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(viewer) = &mut this.viewer {
                                    viewer.error = None;
                                }
                                this.request_viewer_image();
                                cx.notify();
                            })),
                    )
                })
                .into_any_element(),
        };
        let name = if item.reference.name.is_empty() {
            t!("common.image").into_owned()
        } else {
            item.reference.name.clone()
        };
        div()
            .id("image-viewer")
            .size_full()
            .occlude()
            .flex()
            .flex_col()
            .bg(rgb(0x141414))
            .track_focus(&self.viewer_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_image(window, cx),
                    "left" => this.navigate_image(-1),
                    "right" => this.navigate_image(1),
                    "+" | "=" => {
                        if let Some(viewer) = &mut this.viewer {
                            viewer.zoom_by(1.25);
                        }
                    }
                    "-" => {
                        if let Some(viewer) = &mut this.viewer {
                            viewer.zoom_by(0.8);
                        }
                    }
                    "0" => {
                        if let Some(viewer) = &mut this.viewer {
                            viewer.reset();
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                ferrsphen::window_chrome::title_bar(cx)
                    .pl(ferrsphen::window_chrome::leading_inset(window))
                    .gap_3()
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(name)),
            )
            .child(
                div()
                    .id("image-canvas")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        let delta = f32::from(event.delta.pixel_delta(px(24.)).y);
                        if let Some(viewer) = &mut this.viewer {
                            viewer.zoom_by((delta * 0.005).exp());
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if let Some(viewer) = &mut this.viewer {
                                if event.click_count == 2 {
                                    viewer.reset();
                                } else {
                                    viewer.dragging =
                                        Some((event.position.x.into(), event.position.y.into()));
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if let Some(viewer) = &mut this.viewer
                            && let Some((x, y)) = viewer.dragging
                        {
                            let current =
                                (f32::from(event.position.x), f32::from(event.position.y));
                            viewer.pan.0 += current.0 - x;
                            viewer.pan.1 += current.1 - y;
                            viewer.dragging = Some(current);
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            if let Some(viewer) = &mut this.viewer {
                                viewer.dragging = None;
                            }
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            if let Some(viewer) = &mut this.viewer {
                                viewer.dragging = None;
                            }
                        }),
                    )
                    .child(image),
            )
            .child(
                div()
                    .h(rems(3.5))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        Button::new("viewer-previous")
                            .ghost()
                            .icon(gpui_kit::assets::IconName::ChevronLeft)
                            .tooltip(t!("image.previous"))
                            .accessibility_label(t!("image.previous"))
                            .disabled(viewer.index == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.navigate_image(-1);
                                cx.notify();
                            })),
                    )
                    .child(div().text_sm().child(format!(
                        "{} / {}",
                        viewer.index + 1,
                        viewer.images.len()
                    )))
                    .child(
                        Button::new("viewer-next")
                            .ghost()
                            .icon(gpui_kit::assets::IconName::ChevronRight)
                            .tooltip(t!("image.next"))
                            .accessibility_label(t!("image.next"))
                            .disabled(viewer.index + 1 == viewer.images.len())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.navigate_image(1);
                                cx.notify();
                            })),
                    )
                    .child(div().w_4())
                    .child(
                        Button::new("viewer-zoom-out")
                            .ghost()
                            .icon(gpui_kit::assets::IconName::ZoomOut)
                            .tooltip(t!("image.zoom_out"))
                            .accessibility_label(t!("image.zoom_out"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(viewer) = &mut this.viewer {
                                    viewer.zoom_by(0.8);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("viewer-fit")
                            .ghost()
                            .label(format!("{:.0}%", viewer.zoom * 100.))
                            .tooltip(t!("image.fit"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(viewer) = &mut this.viewer {
                                    viewer.reset();
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("viewer-zoom-in")
                            .ghost()
                            .icon(gpui_kit::assets::IconName::ZoomIn)
                            .tooltip(t!("image.zoom_in"))
                            .accessibility_label(t!("image.zoom_in"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(viewer) = &mut this.viewer {
                                    viewer.zoom_by(1.25);
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
    fn overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.details.clone().unwrap_or_default();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background.opacity(0.9))
            .child(
                div()
                    .w_96()
                    .max_h(rems(36.))
                    .flex()
                    .flex_col()
                    .bg(cx.theme().muted)
                    .rounded_lg()
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1().text_sm().child(t!("detail.title")))
                            .child(
                                Button::new("close-details")
                                    .ghost()
                                    .small()
                                    .label(t!("common.close"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.forward = None;
                                        this.details = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .id("details")
                            .min_h_0()
                            .overflow_y_scroll()
                            .text_sm()
                            .child(text),
                    ),
            )
            .into_any_element()
    }
}
impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_desktop_window(window, cx);
        let phone = self.mobile_platform && phone_layout(window.viewport_size());
        if self.phone != phone {
            self.phone = phone;
            self.transition = None;
            self.scroll_anchor = None;
            self.bounce = Bounce::default();
            self.composer
                .update(cx, |input, cx| input.set_submit_on_enter(!phone, cx));
        }
        self.sync_favorites_refresh(cx);
        self.advance_transition(window, cx);
        self.animate_recalls(cx);
        self.visible_bubbles.borrow_mut().clear();
        self.advance_timeline_anchor(window);
        self.advance_reply_jump(false, window, cx);
        if cx.reduce_motion() {
            self.bounce = Bounce::default();
        }
        let content = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .when(!self.phone, |view| {
                view.child(if matches!(self.phase, Phase::Chat) {
                    div()
                        .debug_selector(|| "desktop-title-bar".into())
                        .flex_shrink_0()
                        .child(self.topbar(window, cx))
                        .into_any_element()
                } else {
                    ferrsphen::window_chrome::title_bar(cx)
                        .child(div().flex_1())
                        .into_any_element()
                })
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .px_4()
                        .py_2()
                        .gap_3()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                        .child(
                            Button::new("dismiss-error")
                                .ghost()
                                .small()
                                .label(t!("common.close"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .map(|view| {
                if matches!(self.phase, Phase::Chat) {
                    view.child(self.main_page(window, cx))
                } else {
                    view.child(self.login_view(cx))
                        .when(self.challenge.is_some(), |view| {
                            view.child(self.challenge_view(cx))
                        })
                }
            })
            .when(self.details.is_some(), |view| view.child(self.overlay(cx)))
            .when(self.confirmation.is_some(), |view| {
                view.child(self.confirmation_view(cx))
            });
        ferrsphen::window_chrome::surface(content, window, cx)
    }
}
fn message_key(message: &Message) -> String {
    let nonce = if message.random != 0 {
        message.random
    } else {
        message.message_id as u32
    };
    if message.is_self && nonce != 0 {
        format!("{}:self:{nonce}", conversation_key(&message.conversation))
    } else {
        format!(
            "{}:{}:{}",
            conversation_key(&message.conversation),
            message.sequence,
            message.message_id
        )
    }
}
fn image_message_offset(messages: &[Message], position: usize) -> f32 {
    messages
        .iter()
        .take(position)
        .flat_map(|message| &message.elements)
        .filter_map(|element| match element {
            MessageElement::Image(image) => Some(
                ferrsphen::card_preview::fitted(image.width, image.height).height
                    + if messages.len() > 1 {
                        ferrsphen::card_preview::ACTION_HEIGHT
                    } else {
                        0.
                    }
                    + 8.,
            ),
            _ => None,
        })
        .sum()
}
fn card_key(source: &str, slot: usize, width: u32, height: u32) -> String {
    format!("card-sheet:v4:{source}:{slot}:{width}:{height}")
}
fn image_key(message: &Message, index: usize) -> String {
    format!("preview:{}:{index}", message_key(message))
}
fn clock_time(time: i64) -> String {
    chrono::DateTime::from_timestamp(time, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn recalled_size(window: &Window, cx: &App) -> gpui_kit::Size<Pixels> {
    let label: SharedString = t!("common.recalled").into();
    let font_size = window.rem_size() * 0.875;
    let line = window.text_system().shape_line(
        label.clone(),
        font_size,
        &[TextRun {
            len: label.len(),
            font: Font {
                family: cx.theme().font_family.clone(),
                ..Default::default()
            },
            color: cx.theme().foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    size(px(f32::from(line.width).ceil() + 24.), px(36.))
}

#[cfg(all(test, feature = "ui-tests"))]
mod ui_tests {
    use super::{AppView, DetailKind, Page, Phase, recalled_size};
    use atri_qq::{
        media::{MediaConversation, MediaKind, MediaReference},
        messages::{Conversation, Message, MessageElement, Reply},
    };
    use ferrsphen::backend::{AttachmentKind, Command, ComposePart, Update};
    use ferrsphen::model::Cursor;
    use gpui::{
        AnyWindowHandle, AppContext, Bounds, Entity, TestAppContext, VisualTestContext,
        WindowBounds, WindowOptions, point, px, size,
    };
    use gpui::{PlatformInput, ScrollDelta, ScrollWheelEvent, TouchPhase};
    use std::time::Duration;
    use tokio::sync::mpsc;

    type Fixture = (
        AnyWindowHandle,
        Entity<AppView>,
        mpsc::Sender<Update>,
        mpsc::Receiver<Command>,
    );
    fn fixture(cx: &mut TestAppContext) -> Fixture {
        let (updates, receiver) = mpsc::channel(32);
        let (commands, command_receiver) = mpsc::channel(128);
        let (window, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            ferrsphen::window_chrome::init(cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1240.), px(820.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = AppView::new(commands, receiver, window, cx);
                        view.mobile_platform = true;
                        view.desktop_windows = false;
                        view
                    })
                },
            )
            .unwrap()
        });
        (window, view, updates, command_receiver)
    }
    fn draw(cx: &mut TestAppContext, handle: AnyWindowHandle) {
        cx.update_window(handle, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }
    fn text_message(sequence: u64) -> Message {
        Message {
            conversation: Conversation::Group { uin: 42 },
            sender_uin: 7,
            sender_uid: "peer".into(),
            recipient_uin: 42,
            recipient_uid: String::new(),
            sender_name: "昵称".into(),
            sequence,
            client_sequence: sequence,
            message_id: sequence,
            random: 0,
            time: 1700000000 + sequence as i64,
            is_self: false,
            is_bot: None,
            elements: vec![MessageElement::Text(format!("消息 {sequence}"))],
            metadata: vec![],
            raw: vec![],
        }
    }
    fn picture_message(sequence: u64, width: u32, height: u32) -> Message {
        let mut message = text_message(sequence);
        let mut image = MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 42 });
        image.width = width;
        image.height = height;
        message.elements = vec![MessageElement::Image(image)];
        message
    }
    fn frames(cx: &mut TestAppContext, handle: AnyWindowHandle, count: usize) {
        for _ in 0..count {
            cx.background_executor
                .advance_clock(Duration::from_millis(16));
            cx.update_window(handle, |_, window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    }
    fn wheel(
        cx: &mut TestAppContext,
        handle: AnyWindowHandle,
        position: gpui::Point<gpui::Pixels>,
        delta: f32,
        phase: TouchPhase,
    ) {
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    touch_phase: phase,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
    }
    fn draft_image_file(root: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = root.join(name);
        image::RgbaImage::from_pixel(32, 24, image::Rgba([90, 130, 180, 255]))
            .save(&path)
            .unwrap();
        path
    }
    fn drop_files(cx: &mut TestAppContext, main: AnyWindowHandle, paths: Vec<std::path::PathBuf>) {
        draw(cx, main);
        let position = VisualTestContext::from_window(main, cx)
            .debug_bounds("chat-drop-target")
            .unwrap()
            .center();
        cx.update_window(main, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::FileDrop(gpui::FileDropEvent::Entered {
                    position,
                    paths: gpui::ExternalPaths(paths.into_iter().collect()),
                }),
                cx,
            );
        })
        .unwrap();
        draw(cx, main);
        cx.update_window(main, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::FileDrop(gpui::FileDropEvent::Submit { position }),
                cx,
            );
            window.dispatch_event(PlatformInput::FileDrop(gpui::FileDropEvent::Ended), cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    fn poke_friend(uin: u64, uid: &str, nickname: &str) -> atri_qq::contacts::Friend {
        atri_qq::contacts::Friend {
            uin,
            uid: uid.into(),
            nickname: nickname.into(),
            remark: String::new(),
            personal_sign: String::new(),
            qid: String::new(),
            age: 0,
            gender: atri_qq::contacts::Gender::Unknown(0),
            category_id: 0,
            category: None,
        }
    }
    fn private_poke(actor: u64, target: u64) -> Update {
        Update::Notification(atri_qq::messages::PushEvent::Poke {
            group_uin: None,
            actor_uin: Some(actor),
            target_uin: Some(target),
            parameters: [
                ("action_str".into(), "拍了拍".into()),
                ("suffix_str".into(), "的肩膀".into()),
            ]
            .into_iter()
            .collect(),
            raw: vec![],
        })
    }
    fn reaction_push(
        group: u64,
        sequence: u64,
        code: &str,
        added: bool,
        count: u32,
        operator: &str,
    ) -> crate::activity::ActivityRecord {
        crate::activity::ActivityRecord::notification(atri_qq::messages::PushEvent::Reaction {
            group_uin: group,
            sequence,
            code: code.into(),
            added,
            count,
            operator_uid: operator.into(),
        })
    }
    fn reaction_ack(code: &str, added: bool) -> crate::activity::ActivityRecord {
        crate::activity::ActivityRecord::new(crate::activity::ActivityEvent::ReactionConfirmed {
            group: 42,
            sequence: 1,
            code: code.into(),
            added,
            operator_uid: "self".into(),
        })
    }
    #[gpui::test]
    fn expanded_picture_messages_keep_individual_reaction_rows_between_images(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(picture_message(1, 32, 24), false);
                view.model.receive(picture_message(2, 160, 100), false);
                for code in ["76", "66", "14"] {
                    view.apply(Update::Activity(reaction_push(
                        42, 1, code, true, 1, "peer",
                    )));
                }
                view.apply(Update::Activity(reaction_push(
                    42, 2, "76", true, 2, "peer",
                )));
                view.change_stack("group:42:1:1", true, cx);
            })
        });
        frames(cx, main, 70);
        let mut visual = VisualTestContext::from_window(main, cx);
        let first = visual.debug_bounds("stack-card:group:42:1:1:0").unwrap();
        let second = visual.debug_bounds("stack-card:group:42:1:1:1").unwrap();
        let first_chip = visual.debug_bounds("reaction:42:1:76").unwrap();
        let second_chip = visual.debug_bounds("reaction:42:2:76").unwrap();
        assert!(first_chip.top() >= first.bottom());
        assert!(
            first_chip.bottom() <= second.top(),
            "{first_chip:?}, {second:?}"
        );
        assert!(second_chip.top() >= second.bottom());
    }
    #[gpui::test]
    fn group_reaction_chips_toggle_and_do_not_double_count_receipts_and_pushes(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.profile = Some(atri_qq::UserInfo {
                    uin: 100,
                    uid: "self".into(),
                    nickname: "我".into(),
                    age: 0,
                    gender: 0,
                });
                view.model.receive(text_message(1), false);
                view.apply(Update::Activity(reaction_push(
                    42, 1, "76", true, 3, "peer",
                )));
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let chip = visual.debug_bounds("reaction:42:1:76").unwrap();
        visual.simulate_click(chip.center(), Default::default());
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.toggle_reaction(&text_message(1), "76".into(), cx)
            })
        });
        let sends = std::iter::from_fn(|| commands.try_recv().ok()).filter(|command| matches!(command, Command::Reaction { group:42, sequence:1, code, added:true } if code == "76")).count();
        assert_eq!(sends, 1);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Activity(reaction_push(
                    42, 1, "76", true, 4, "self",
                )));
                view.apply(Update::ReactionDone {
                    group: 42,
                    sequence: 1,
                    code: "76".into(),
                    result: Ok(reaction_ack("76", true)),
                });
                let state = &view.model.reactions[&(42, 1, "76".into())];
                assert_eq!(state.count, 4);
                assert!(state.actors.contains("self"));
                assert!(view.pending_reactions.is_empty());
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let chip = visual.debug_bounds("reaction:42:1:76").unwrap();
        visual.simulate_click(chip.center(), Default::default());
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(
            |command| matches!(command, Command::Reaction { added:false, code, .. } if code == "76")
        ));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::ReactionDone {
                    group: 42,
                    sequence: 1,
                    code: "76".into(),
                    result: Ok(reaction_ack("76", false)),
                });
                view.apply(Update::Activity(reaction_push(
                    42, 1, "76", false, 3, "self",
                )));
                let state = &view.model.reactions[&(42, 1, "76".into())];
                assert_eq!(state.count, 3);
                assert!(!state.actors.contains("self"));
                view.toggle_reaction(&text_message(1), "66".into(), cx);
                view.apply(Update::ReactionDone {
                    group: 42,
                    sequence: 1,
                    code: "66".into(),
                    result: Err("server detail".into()),
                });
                assert!(!view.model.reactions.contains_key(&(42, 1, "66".into())));
                assert!(view.pending_reactions.is_empty());
                assert_eq!(view.error.as_deref(), Some("server detail"));
                let mut private = text_message(2);
                private.conversation = Conversation::Friend {
                    uin: 200,
                    uid: "peer".into(),
                };
                view.toggle_reaction(&private, "76".into(), cx);
                assert!(!view.can_react(&private, false));
            })
        });
    }
    #[gpui::test]
    fn phone_context_menu_reactions_use_group_api_without_a_message_button(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(390.), px(844.)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.phone_conversation = true;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("react:group:42:1:1").is_none());
        let bubble = visual.debug_bounds("message-bubble:group:42:1:1").unwrap();
        visual.simulate_mouse_down(
            bubble.center(),
            gpui::MouseButton::Right,
            Default::default(),
        );
        visual.simulate_mouse_up(
            bubble.center(),
            gpui::MouseButton::Right,
            Default::default(),
        );
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("reaction-panel").is_some());
        let choice = visual.debug_bounds("reaction-choice:66").unwrap();
        visual.simulate_click(choice.center(), Default::default());
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|command| matches!(command, Command::Reaction { group:42, sequence:1, code, added:true } if code == "66")));
        assert_eq!(cx.windows().len(), 1);
    }
    #[gpui::test]
    fn desktop_context_reactions_are_two_rows_and_toggle_own_selection(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.profile = Some(atri_qq::UserInfo {
                    uin: 100,
                    uid: "self".into(),
                    nickname: "我".into(),
                    age: 0,
                    gender: 0,
                });
                view.model.receive(text_message(1), false);
                view.apply(Update::Activity(reaction_push(
                    42, 1, "66", true, 1, "self",
                )));
                cx.notify();
            });
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let bubble = visual.debug_bounds("message-bubble:group:42:1:1").unwrap();
        visual.simulate_mouse_move(bubble.center(), None, Default::default());
        frames(cx, main, 8);
        assert!(visual.debug_bounds("react:group:42:1:1").is_none());
        visual.simulate_mouse_down(
            bubble.center(),
            gpui::MouseButton::Right,
            Default::default(),
        );
        visual.simulate_mouse_up(
            bubble.center(),
            gpui::MouseButton::Right,
            Default::default(),
        );
        frames(cx, main, 3);
        let heart = visual.debug_bounds("reaction-choice:66").unwrap();
        let question = visual.debug_bounds("reaction-choice:32").unwrap();
        let laugh = visual.debug_bounds("reaction-choice:28").unwrap();
        let victory = visual.debug_bounds("reaction-choice:79").unwrap();
        assert_eq!(heart.origin.y, question.origin.y);
        assert_eq!(laugh.origin.y, victory.origin.y);
        assert!(laugh.origin.y > heart.origin.y);
        assert_eq!(heart.origin.x, laugh.origin.x);
        assert!(visual.debug_bounds("message-menu-reply").unwrap().origin.y > victory.origin.y);
        visual.simulate_click(heart.center(), Default::default());
        frames(cx, main, 3);
        assert!(visual.debug_bounds("reaction-panel").is_none());
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(|command| matches!(command, Command::Reaction { group:42, sequence:1, code, added:false } if code == "66")));
        assert_eq!(cx.windows().len(), 1);
    }
    #[gpui::test]
    fn restored_pokes_keep_their_time_position_and_reactions_restore_without_duplicate_events(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        let mut restored = ferrsphen::model::Model::default();
        let first = text_message(1);
        let second = text_message(2);
        restored.receive(first.clone(), false);
        restored.receive(second, false);
        let mut poke =
            crate::activity::ActivityRecord::notification(atri_qq::messages::PushEvent::Poke {
                group_uin: Some(42),
                actor_uin: Some(7),
                target_uin: Some(100),
                parameters: Default::default(),
                raw: vec![],
            });
        poke.time = first.time * 1000 + 500;
        restored.apply_activity(poke.clone());
        let reaction = reaction_push(42, 1, "76", true, 4, "self");
        restored.apply_activity(reaction.clone());
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.profile = Some(atri_qq::UserInfo {
                    uin: 100,
                    uid: "self".into(),
                    nickname: "我".into(),
                    age: 0,
                    gender: 0,
                });
                view.apply(Update::Restored(Box::new(restored)));
                view.apply(Update::Activity(poke));
                view.apply(Update::Activity(reaction));
                let notices = &view.poke_notices["group:42"];
                assert_eq!(notices.len(), 1);
                assert_eq!(notices[0].0.as_ref(), Some(&super::message_key(&first)));
                assert_eq!(view.model.reactions[&(42, 1, "76".into())].count, 4);
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("poke-notice").is_some());
        assert!(visual.debug_bounds("reaction:42:1:76").is_some());
    }
    #[gpui::test]
    fn private_poke_pushes_display_both_directions_in_uid_only_chats(cx: &mut TestAppContext) {
        let (main, view, updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.profile = Some(atri_qq::UserInfo {
                    uin: 100,
                    uid: "self".into(),
                    nickname: "我".into(),
                    age: 0,
                    gender: 0,
                });
                let mut message = text_message(1);
                message.conversation = Conversation::Friend {
                    uin: 0,
                    uid: "peer".into(),
                };
                message.sender_uin = 0;
                view.model.receive(message, false);
                view.model
                    .contacts(vec![poke_friend(200, "peer", "好友昵称")], vec![]);
                assert!(matches!(
                    view.model.selected_chat().unwrap().conversation,
                    Conversation::Friend { uin: 0, .. }
                ));
                cx.notify();
            })
        });
        updates.try_send(private_poke(100, 200)).unwrap();
        updates.try_send(private_poke(200, 100)).unwrap();
        cx.run_until_parked();
        frames(cx, main, 3);
        cx.update(|cx| {
            let view = view.read(cx);
            let notices = &view.poke_notices["dm:uid:peer"];
            assert_eq!(notices.len(), 2);
            assert_eq!(notices[0].1, "You 拍了拍 好友昵称的肩膀");
            assert_eq!(notices[1].1, "好友昵称 拍了拍 You的肩膀");
            assert!(notices.iter().all(|(after, _)| after.is_some()));
            assert!(view.pending_pokes.is_empty());
        });
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("poke-notice")
                .is_some()
        );
    }
    #[gpui::test]
    fn private_poke_waits_for_identity_and_displays_without_message_history(
        cx: &mut TestAppContext,
    ) {
        let (main, view, updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.profile = Some(atri_qq::UserInfo {
                    uin: 100,
                    uid: "self".into(),
                    nickname: "我".into(),
                    age: 0,
                    gender: 0,
                });
                view.model.ensure(
                    Conversation::Friend {
                        uin: 0,
                        uid: "peer".into(),
                    },
                    "好友昵称".into(),
                );
                cx.notify();
            })
        });
        updates.try_send(private_poke(200, 100)).unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).pending_pokes.len(), 1));
        updates
            .try_send(Update::UserUin {
                uid: "peer".into(),
                result: Ok(200),
            })
            .unwrap();
        cx.run_until_parked();
        frames(cx, main, 3);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.pending_pokes.is_empty());
            let notices = &view.poke_notices["dm:uid:peer"];
            assert_eq!(notices.len(), 1);
            assert_eq!(notices[0].0, None);
            assert_eq!(notices[0].1, "好友昵称 拍了拍 You的肩膀");
        });
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("poke-notice")
                .is_some()
        );
        updates
            .try_send(Update::Contacts {
                friends: vec![poke_friend(200, "peer", "好友昵称")],
                groups: vec![],
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).poke_notices["dm:uid:peer"].len(), 1));
    }
    #[gpui::test]
    fn message_text_can_be_drag_selected_and_copied_without_changing_the_draft(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        let text = "第一段文字 alpha beta 第二段";
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                message.elements = vec![MessageElement::Text(text.into())];
                view.model.receive(message, false);
                view.composer.update(cx, |input, cx| {
                    input.set_value("保留草稿", window, cx);
                    input.focus(window, cx);
                });
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let bounds = visual.debug_bounds("message-text:group:42:1:0").unwrap();
        let start = point(bounds.left() + px(1.), bounds.center().y);
        let end = point(bounds.left() + bounds.size.width / 3., bounds.center().y);
        visual.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, Some(gpui::MouseButton::Left), Default::default());
        visual.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
        frames(cx, main, 3);
        let selected = cx
            .update_window(main, |_, window, cx| {
                gpui_kit::base::TextSelection::selected_text(window, cx)
            })
            .unwrap();
        assert!(!selected.is_empty() && selected != text, "{selected:?}");
        assert!(text.contains(&selected));
        clipboard_key(cx, main, "c");
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(selected.as_str())
            );
            assert_eq!(view.read(cx).composer.read(cx).value().as_ref(), "保留草稿");
        });
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.commit_select(Conversation::Group { uin: 99 }, window, cx);
            })
        })
        .unwrap();
        cx.update_window(main, |_, window, cx| {
            assert!(gpui_kit::base::TextSelection::selected_text(window, cx).is_empty())
        })
        .unwrap();
    }
    #[gpui::test]
    fn phone_message_long_press_selects_text_and_handles_adjust_and_copy_it(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(390.), px(844.)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.phone_conversation = true;
                let mut message = text_message(1);
                message.elements = vec![MessageElement::Text("alpha beta gamma".into())];
                view.model.receive(message, false);
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let bounds = visual.debug_bounds("message-text:group:42:1:0").unwrap();
        let start = point(bounds.left() + px(10.), bounds.center().y);
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            visual.simulate_event(gpui::LongPressEvent {
                phase,
                start_position: start,
                position: start,
            });
        }
        frames(cx, main, 3);
        let snapshot = cx
            .update_window(main, |_, window, cx| {
                assert_eq!(
                    gpui_kit::base::TextSelection::selected_text(window, cx),
                    "alpha"
                );
                let snapshot = gpui_kit::base::TextSelection::touch_selection(window, cx).unwrap();
                assert!(snapshot.is_menu_open());
                snapshot
            })
            .unwrap();
        let caret = snapshot.end();
        let handle = point(caret.left(), caret.bottom() + px(3.));
        let destination = point(bounds.right() + px(1.), handle.y);
        let mut visual = VisualTestContext::from_window(main, cx);
        visual.simulate_event(gpui::TouchDragEvent {
            phase: TouchPhase::Started,
            start_position: handle,
            position: handle,
        });
        for phase in [TouchPhase::Moved, TouchPhase::Ended] {
            visual.simulate_event(gpui::TouchDragEvent {
                phase,
                start_position: handle,
                position: destination,
            });
        }
        frames(cx, main, 3);
        cx.update_window(main, |_, window, cx| {
            assert_eq!(
                gpui_kit::base::TextSelection::selected_text(window, cx),
                "alpha beta gamma"
            );
            assert!(
                gpui_kit::base::TextSelection::touch_selection(window, cx)
                    .unwrap()
                    .is_menu_open()
            );
        })
        .unwrap();
        clipboard_key(cx, main, "c");
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("alpha beta gamma")
            )
        });
    }
    #[gpui::test]
    fn forwarded_message_text_keeps_literal_content_and_supports_selection(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        let text = "**literal** <tag> & text";
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                let mut message = text_message(1);
                message.elements = vec![MessageElement::Text(text.into())];
                view.forward = Some(vec![message]);
                view.open_detail_window(DetailKind::Forward, cx);
            })
        });
        cx.run_until_parked();
        let forward = cx.update(|cx| view.read(cx).forward_window.unwrap());
        frames(cx, forward, 3);
        let mut visual = VisualTestContext::from_window(forward, cx);
        let bounds = visual.debug_bounds("message-text:group:42:1:0").unwrap();
        let start = point(bounds.left(), bounds.center().y);
        let end = point(bounds.right(), bounds.center().y);
        visual.simulate_mouse_down(start, gpui::MouseButton::Left, Default::default());
        visual.simulate_mouse_move(end, Some(gpui::MouseButton::Left), Default::default());
        visual.simulate_mouse_up(end, gpui::MouseButton::Left, Default::default());
        frames(cx, forward, 3);
        clipboard_key(cx, forward, "c");
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(text)
            )
        });
        assert_eq!(cx.windows().len(), 2);
        draw(cx, main);
    }
    #[gpui::test]
    fn favorites_refresh_on_reopen_and_every_thirty_seconds_only_while_visible(
        cx: &mut TestAppContext,
    ) {
        fn requests(commands: &mut mpsc::Receiver<Command>) -> usize {
            std::iter::from_fn(|| commands.try_recv().ok())
                .filter(|command| matches!(command, Command::Favorites))
                .count()
        }
        fn click(cx: &mut TestAppContext, main: AnyWindowHandle, selector: &'static str) {
            let mut visual = VisualTestContext::from_window(main, cx);
            let bounds = visual.debug_bounds(selector).unwrap();
            visual.simulate_click(bounds.center(), Default::default());
            frames(cx, main, 3);
        }
        fn complete(cx: &mut TestAppContext, updates: &mpsc::Sender<Update>) {
            updates
                .try_send(Update::Favorites {
                    cached: false,
                    result: Ok(vec![]),
                })
                .unwrap();
            cx.run_until_parked();
        }
        fn advance(cx: &mut TestAppContext, seconds: u64) {
            cx.background_executor
                .advance_clock(Duration::from_secs(seconds));
            cx.run_until_parked();
        }
        let (main, view, updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        frames(cx, main, 3);
        requests(&mut commands);
        click(cx, main, "emoji-button");
        assert_eq!(requests(&mut commands), 1);
        complete(cx, &updates);
        advance(cx, 31);
        assert_eq!(requests(&mut commands), 0);
        click(cx, main, "favorites-tab");
        assert_eq!(requests(&mut commands), 1);
        complete(cx, &updates);
        advance(cx, 29);
        assert_eq!(requests(&mut commands), 0);
        advance(cx, 1);
        assert_eq!(requests(&mut commands), 1);
        advance(cx, 30);
        assert_eq!(
            requests(&mut commands),
            0,
            "an in-flight refresh must not be duplicated"
        );
        updates
            .try_send(Update::Favorites {
                cached: false,
                result: Err("retry".into()),
            })
            .unwrap();
        cx.run_until_parked();
        advance(cx, 30);
        assert_eq!(requests(&mut commands), 1);
        complete(cx, &updates);
        click(cx, main, "faces-tab");
        advance(cx, 60);
        assert_eq!(requests(&mut commands), 0);
        click(cx, main, "emoji-button");
        cx.update(|cx| assert!(!view.read(cx).emoji_open));
        advance(cx, 60);
        assert_eq!(requests(&mut commands), 0);
        click(cx, main, "emoji-button");
        assert_eq!(requests(&mut commands), 1, "reopening must fetch again");
        complete(cx, &updates);
        click(cx, main, "favorites-tab");
        assert_eq!(requests(&mut commands), 1);
        complete(cx, &updates);
        click(cx, main, "emoji-button");
        advance(cx, 60);
        assert_eq!(requests(&mut commands), 0);
        cx.update(|cx| assert!(view.read(cx).favorites_refresh.is_none()));
    }
    #[gpui::test]
    fn desktop_login_and_connecting_share_small_window_then_open_main_window(
        cx: &mut TestAppContext,
    ) {
        let (login, view, updates, mut commands) = fixture(cx);
        cx.simulate_window_resize(login, crate::window_chrome::AppWindowKind::Login.size());
        cx.update_window(login, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.mobile_platform = false;
                view.desktop_windows = true;
                view.account
                    .update(cx, |input, cx| input.set_value("12345", window, cx));
                cx.notify();
            })
        })
        .unwrap();
        draw(cx, cx.windows()[0]);
        assert_eq!(cx.windows().len(), 1);
        updates.try_send(Update::LoginRequired).unwrap();
        cx.run_until_parked();
        draw(cx, cx.windows()[0]);
        let mut visual = VisualTestContext::from_window(login, cx);
        assert!(visual.debug_bounds("password-login").is_some());
        assert!(visual.debug_bounds("qr-login").is_some());
        assert!(visual.debug_bounds("switch-login").is_none());
        updates.try_send(Update::Connecting).unwrap();
        cx.run_until_parked();
        draw(cx, cx.windows()[0]);
        assert_eq!(cx.windows()[0].window_id(), login.window_id());
        updates
            .try_send(Update::Online(atri_qq::UserInfo {
                uin: 12345,
                uid: "self".into(),
                nickname: "我".into(),
                age: 0,
                gender: 0,
            }))
            .unwrap();
        updates.try_send(Update::Message(text_message(1))).unwrap();
        cx.run_until_parked();
        draw(cx, cx.windows()[0]);
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        let main = cx.windows()[0];
        assert_ne!(main.window_id(), login.window_id());
        draw(cx, cx.windows()[0]);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(
                    window.viewport_size(),
                    crate::window_chrome::AppWindowKind::Main.size()
                );
                assert!(!view.phone);
                view.composer.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.replace("切换后发送", window, cx);
                });
            })
        })
        .unwrap();
        cx.simulate_keystrokes(main, "enter");
        assert!(
            std::iter::from_fn(|| commands.try_recv().ok())
                .any(|command| matches!(command, Command::Send { .. }))
        );
        updates.try_send(Update::LoginRequired).unwrap();
        cx.run_until_parked();
        draw(cx, cx.windows()[0]);
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        let next_login = cx.windows()[0];
        assert_ne!(next_login.window_id(), main.window_id());
        draw(cx, next_login);
        cx.update_window(next_login, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(
                    window.viewport_size(),
                    crate::window_chrome::AppWindowKind::Login.size()
                );
                assert_eq!(view.account.read(cx).value().as_ref(), "12345");
                view.password.update(cx, |input, cx| {
                    input.set_value("password", window, cx);
                    input.focus(window, cx);
                });
            })
        })
        .unwrap();
        cx.simulate_keystrokes(next_login, "enter");
        assert!(
            std::iter::from_fn(|| commands.try_recv().ok())
                .any(|command| matches!(command, Command::Login { uin: 12345, .. }))
        );
        draw(cx, next_login);
        assert_eq!(cx.windows()[0].window_id(), next_login.window_id());
    }
    #[gpui::test]
    fn desktop_never_enters_phone_layout_when_resized_below_phone_breakpoint(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.mobile_platform = false;
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        cx.simulate_window_resize(main, size(px(500.), px(450.)));
        draw(cx, main);
        cx.update(|cx| assert!(!view.read(cx).phone));
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("desktop-title-bar").is_some());
        assert!(visual.debug_bounds("chat-list").is_some());
        assert!(visual.debug_bounds("conversation-panel").is_some());
        assert_eq!(
            visual.debug_bounds("navigation").unwrap().size.width,
            px(52.)
        );
    }
    #[gpui::test]
    fn expression_popup_inserts_faces_and_favorites_into_one_ordered_draft(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, mut commands) = fixture(cx);
        let root = tempfile::tempdir().unwrap();
        let image = draft_image_file(root.path(), "favorite.png");
        let extensionless = root.path().join("cached-favorite");
        std::fs::copy(&image, &extensionless).unwrap();
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.composer
                    .update(cx, |input, cx| input.replace("前", window, cx));
                view.apply(Update::Favorites {
                    cached: true,
                    result: Ok(vec![crate::expressions::Favorite {
                        id: "ABC".into(),
                        url: "https://example.test/favorite".into(),
                    }]),
                });
                view.assets
                    .insert("favorite:0:ABC".into(), extensionless.clone());
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, main, 3);
        let initial_windows = cx.update(|cx| cx.windows().len());
        let mut visual = VisualTestContext::from_window(main, cx);
        let button = visual.debug_bounds("emoji-button").unwrap();
        visual.simulate_click(button.center(), Default::default());
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let panel = visual
            .debug_bounds("emoji-panel")
            .expect("popup is in the main window");
        assert!(panel.bottom() <= button.top() + px(1.));
        assert_eq!(cx.update(|cx| cx.windows().len()), initial_windows);
        let face = visual.debug_bounds("face-choice:14").unwrap();
        visual.simulate_click(face.center(), Default::default());
        frames(cx, main, 3);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.composer.read(cx).tokens().len(), 1);
                assert!(!view.emoji_open);
                assert!(view.compose_images.is_empty());
                view.composer
                    .update(cx, |input, cx| input.replace("中", window, cx));
            })
        })
        .unwrap();
        let mut visual = VisualTestContext::from_window(main, cx);
        let button = visual.debug_bounds("emoji-button").unwrap();
        visual.simulate_click(button.center(), Default::default());
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let tab = visual.debug_bounds("favorites-tab").unwrap();
        visual.simulate_click(tab.center(), Default::default());
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let favorite = visual.debug_bounds("favorite:0:ABC").unwrap();
        visual.simulate_click(favorite.center(), Default::default());
        frames(cx, main, 3);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(!view.emoji_open);
                assert_eq!(view.composer.read(cx).tokens().len(), 2);
                let path = view.compose_images.values().next().unwrap();
                assert_eq!(std::fs::read(path).unwrap(), std::fs::read(&image).unwrap());
                view.composer
                    .update(cx, |input, cx| input.replace("后", window, cx));
                view.send(window, cx);
            })
        })
        .unwrap();
        let mut sent = Vec::new();
        let mut favorites = 0;
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Compose { parts, .. } => sent.push(parts),
                Command::Favorites => favorites += 1,
                Command::Send { .. } => panic!("draft was split"),
                _ => {}
            }
        }
        assert_eq!(favorites, 1);
        assert_eq!(sent.len(), 1);
        let parts = &sent[0];
        assert_eq!(parts.len(), 5);
        assert!(
            matches!(&parts[0], ComposePart::Element(element) if matches!(element.as_ref(), MessageElement::Text(text) if text == "前"))
        );
        assert!(
            matches!(&parts[1], ComposePart::Element(element) if crate::expressions::Face::from_element(element).is_some_and(|face| face.id == 14))
        );
        assert!(
            matches!(&parts[2], ComposePart::Element(element) if matches!(element.as_ref(), MessageElement::Text(text) if text == "中"))
        );
        assert!(matches!(&parts[3], ComposePart::Image(_)));
        assert!(
            matches!(&parts[4], ComposePart::Element(element) if matches!(element.as_ref(), MessageElement::Text(text) if text == "后"))
        );
    }
    #[gpui::test]
    fn phone_expression_popup_fits_without_creating_a_window(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(320.), px(568.)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.phone_conversation = true;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let button = visual.debug_bounds("emoji-button").unwrap();
        visual.simulate_click(button.center(), Default::default());
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let panel = visual.debug_bounds("emoji-panel").unwrap();
        assert!(
            panel.left() >= px(0.) && panel.right() <= px(320.),
            "{panel:?}"
        );
        assert!(
            panel.top() >= px(0.) && panel.bottom() <= px(568.),
            "{panel:?}"
        );
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    }
    #[gpui::test]
    fn avatar_menu_mentions_without_losing_images_and_sends_poke_to_clicked_user(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, mut commands) = fixture(cx);
        let root = tempfile::tempdir().unwrap();
        let image = draft_image_file(root.path(), "draft.png");
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.insert_image(image.clone(), window, cx);
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let avatar = visual.debug_bounds("participant-avatar:7").unwrap();
        let press = avatar.center();
        visual.simulate_mouse_down(press, gpui::MouseButton::Right, Default::default());
        visual.simulate_mouse_up(press, gpui::MouseButton::Right, Default::default());
        draw(cx, main);
        let at = press + point(px(30.), px(17.));
        visual.simulate_mouse_move(at, None, Default::default());
        visual.simulate_click(at, Default::default());
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.composer.read(cx).tokens().len(), 1);
            assert_eq!(view.mentions[0].uin, 7);
            assert_eq!(view.mentions[0].display, "昵称");
            assert!(view.composer.read(cx).value().contains("@昵称 "));
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let press = visual
            .debug_bounds("participant-avatar:7")
            .unwrap()
            .center();
        visual.simulate_mouse_down(press, gpui::MouseButton::Right, Default::default());
        visual.simulate_mouse_up(press, gpui::MouseButton::Right, Default::default());
        draw(cx, main);
        let poke = press + point(px(30.), px(45.));
        visual.simulate_mouse_move(poke, None, Default::default());
        visual.simulate_click(poke, Default::default());
        let mut targets = Vec::new();
        while let Ok(command) = commands.try_recv() {
            if let Command::Poke {
                conversation,
                target,
                uid,
            } = command
            {
                targets.push((conversation, target, uid));
            }
        }
        assert_eq!(
            targets,
            vec![(Conversation::Group { uin: 42 }, 7, "peer".into())]
        );
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Notification(atri_qq::messages::PushEvent::Poke {
                    group_uin: Some(42),
                    actor_uin: Some(7),
                    target_uin: Some(8),
                    parameters: Default::default(),
                    raw: vec![],
                }));
                assert_eq!(view.poke_notices["group:42"].len(), 1);
                cx.notify();
            })
        });
        draw(cx, main);
    }
    #[gpui::test]
    fn phone_login_defaults_to_password_and_switches_to_qr(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(320.), px(568.)));
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Login;
                view.account
                    .update(cx, |input, cx| input.set_value("12345678", window, cx));
                view.password.update(cx, |input, cx| {
                    input.set_value("saved-password", window, cx)
                });
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let password = visual.debug_bounds("password-login").unwrap();
        let switch = visual.debug_bounds("switch-login").unwrap();
        assert!(switch.top() >= password.bottom());
        assert!(visual.debug_bounds("qr-login").is_none());
        let root = tempfile::tempdir().unwrap();
        let qr = draft_image_file(root.path(), "qr.png");
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Qr {
                    bytes: std::fs::read(qr).unwrap(),
                    url: "qr-login".into(),
                });
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("password-login").is_some());
        assert!(visual.debug_bounds("qr-login").is_none());
        visual.simulate_click(switch.center(), Default::default());
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("password-login").is_none());
        assert!(visual.debug_bounds("qr-login").is_some());
        assert_eq!(
            std::iter::from_fn(|| commands.try_recv().ok())
                .filter(|command| matches!(command, Command::Qr))
                .count(),
            1
        );
        let switch = visual.debug_bounds("switch-login").unwrap();
        visual.simulate_click(switch.center(), Default::default());
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("password-login").is_some());
        assert!(visual.debug_bounds("qr-login").is_none());
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.account.read(cx).value().as_ref(), "12345678");
            assert_eq!(view.password.read(cx).value().as_ref(), "saved-password");
        });
        cx.simulate_window_resize(main, size(px(820.), px(1180.)));
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let qr = visual.debug_bounds("qr-login").unwrap();
        let password = visual.debug_bounds("password-login").unwrap();
        assert!(qr.right() <= password.left());
        assert!(visual.debug_bounds("switch-login").is_none());
    }

    #[gpui::test]
    fn phone_starts_at_list_and_navigates_back_without_losing_draft(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(390.), px(844.)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        frames(cx, main, 3);
        let mut visual = VisualTestContext::from_window(main, cx);
        let list = visual.debug_bounds("chat-list").unwrap();
        let nav = visual.debug_bounds("navigation").unwrap();
        assert_eq!(list.size.width, px(390.));
        assert_eq!(nav.size.width, px(390.));
        assert_eq!(nav.bottom(), px(844.));
        assert!(nav.top() >= list.bottom() - px(1.));
        assert!(visual.debug_bounds("conversation-panel").is_none());
        assert!(visual.debug_bounds("desktop-title-bar").is_none());
        assert!(visual.debug_bounds("group-sidebar").is_none());
        let chat = visual.debug_bounds("chat:group:42").unwrap();
        visual.simulate_click(chat.center(), Default::default());
        frames(cx, main, 25);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("chat-list").is_none());
        assert!(visual.debug_bounds("group-sidebar").is_none());
        assert!(visual.debug_bounds("conversation-panel").is_some());
        assert!(visual.debug_bounds("navigation").is_none());
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.composer
                    .update(cx, |input, cx| input.set_value("保留草稿", window, cx));
            })
        })
        .unwrap();
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let back = visual.debug_bounds("back-to-chats").unwrap();
        visual.simulate_click(back.center(), Default::default());
        frames(cx, main, 3);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Message(text_message(2)));
                view.apply(Update::Message(text_message(2)));
                assert_eq!(view.model.selected_chat().unwrap().unread, 1);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let settings = visual.debug_bounds("nav-settings").unwrap();
        visual.simulate_click(settings.center(), Default::default());
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("settings-page").is_some());
        assert!(visual.debug_bounds("navigation").is_some());
        let chat_nav = visual.debug_bounds("nav-chat").unwrap();
        visual.simulate_click(chat_nav.center(), Default::default());
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let chat = visual.debug_bounds("chat:group:42").unwrap();
        visual.simulate_click(chat.center(), Default::default());
        frames(cx, main, 25);
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.composer.read(cx).value().as_ref(), "保留草稿");
            assert_eq!(view.model.selected_chat().unwrap().unread, 0);
        });
    }

    #[gpui::test]
    fn phone_composer_grows_with_wrapped_lines_and_avoids_keyboard(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(390.), px(844.)));
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.phone_conversation = true;
                view.composer
                    .update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, main, 4);
        let initial = VisualTestContext::from_window(main, cx)
            .debug_bounds("composer-editor")
            .unwrap();
        assert!(
            initial.size.height <= px(48.),
            "single-line composer: {:?}",
            initial.size
        );
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.composer.update(cx, |input, cx| {
                    input.set_value("中文自动换行".repeat(15), window, cx)
                });
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, main, 4);
        let grown = VisualTestContext::from_window(main, cx)
            .debug_bounds("composer-editor")
            .unwrap();
        assert!(grown.size.height > initial.size.height);
        assert!(grown.size.height <= px(180.));
        cx.simulate_keystrokes(main, "enter");
        cx.update(|cx| assert!(view.read(cx).composer.read(cx).value().contains('\n')));
        assert!(
            !std::iter::from_fn(|| commands.try_recv().ok())
                .any(|command| matches!(command, Command::Send { .. }))
        );
        cx.simulate_window_visual_viewport_change(
            main,
            Bounds::new(point(px(0.), px(44.)), size(px(390.), px(500.))),
        );
        frames(cx, main, 4);
        let mut visual = VisualTestContext::from_window(main, cx);
        let safe = visual.debug_bounds("safe-content").unwrap();
        let editor = visual.debug_bounds("composer-editor").unwrap();
        assert_eq!(safe.top(), px(44.));
        assert!(safe.bottom() <= px(544.));
        assert!(editor.bottom() <= safe.bottom());
        assert!(
            visual
                .debug_bounds("conversation-panel")
                .unwrap()
                .size
                .height
                > px(200.)
        );
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| view.send(window, cx))
        })
        .unwrap();
        frames(cx, main, 4);
        let empty = VisualTestContext::from_window(main, cx)
            .debug_bounds("composer-editor")
            .unwrap();
        assert_eq!(empty.size.height, initial.size.height);
    }

    #[gpui::test]
    fn tablet_keeps_desktop_columns_and_phone_landscape_keeps_phone_routes(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.simulate_window_resize(main, size(px(820.), px(1180.)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        frames(cx, main, 4);
        let mut visual = VisualTestContext::from_window(main, cx);
        let nav = visual.debug_bounds("navigation").unwrap();
        assert_eq!(nav.size.width, px(52.));
        assert!(visual.debug_bounds("desktop-title-bar").is_some());
        assert!(visual.debug_bounds("chat-list").is_some());
        assert!(visual.debug_bounds("conversation-panel").is_some());
        assert!(visual.debug_bounds("group-sidebar").is_some());
        cx.simulate_window_resize(main, size(px(844.), px(390.)));
        frames(cx, main, 4);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("desktop-title-bar").is_none());
        assert!(visual.debug_bounds("group-sidebar").is_none());
        assert!(visual.debug_bounds("conversation-panel").is_none());
        assert_eq!(
            visual.debug_bounds("navigation").unwrap().size.width,
            px(844.)
        );
        assert!(visual.debug_bounds("chat-list").is_some());
    }

    #[gpui::test]
    fn send_scroll_tweens_from_old_history_and_tracks_lazy_layout(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=1000 {
                    view.model.receive(text_message(sequence), false);
                }
                cx.notify();
            })
        });
        frames(cx, main, 8);
        cx.update(|cx| view.read(cx).scroll.scroll_to_top_of_item(0));
        frames(cx, main, 8);
        let before = cx.update(|cx| view.read(cx).scroll.offset().y);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.jump_to_reply(
                    Conversation::Group { uin: 42 },
                    Reply::with_preview(&text_message(10)).unwrap(),
                    false,
                    cx,
                );
                view.scroll_anchor = view.capture_timeline_anchor();
                view.composer
                    .update(cx, |input, cx| input.set_value("新的消息", window, cx));
                view.send(window, cx);
                assert!(view.reply_jump.is_none());
                assert!(view.scroll_anchor.is_none());
                assert!(view.send_scroll.is_some());
                assert_eq!(
                    view.scroll.offset().y,
                    before,
                    "send must not jump instantly"
                );
            })
        })
        .unwrap();
        cx.run_until_parked();
        frames(cx, main, 3);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.scroll.offset().y < before);
            assert!(view.scroll.offset().y > -view.scroll.max_offset().y + px(1.));
        });
        let (id, nonce) = std::iter::from_fn(|| commands.try_recv().ok())
            .find_map(|command| match command {
                Command::Send { id, nonce, .. } => Some((id, nonce)),
                _ => None,
            })
            .unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                let mut receipt = text_message(1001);
                receipt.is_self = true;
                receipt.random = nonce;
                receipt.elements = vec![MessageElement::Text("服务器回执".repeat(60))];
                view.apply(Update::Sent {
                    id,
                    result: Ok(receipt),
                });
                cx.notify();
            });
        });
        frames(cx, main, 160);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.send_scroll.is_none());
            assert!(!view.send_scroll_animating);
            assert!((view.scroll.offset().y + view.scroll.max_offset().y).abs() < px(1.));
        });
    }

    #[gpui::test]
    fn attachments_and_retries_scroll_without_mouse_or_frame_callbacks(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=80 {
                    view.model.receive(text_message(sequence), false);
                }
                cx.notify();
            })
        });
        frames(cx, main, 8);
        for kind in [
            AttachmentKind::File,
            AttachmentKind::Voice,
            AttachmentKind::Video,
        ] {
            cx.update(|cx| view.read(cx).scroll.scroll_to_top_of_item(0));
            frames(cx, main, 8);
            cx.update(|cx| {
                view.update(cx, |view, cx| {
                    view.jump_to_reply(
                        Conversation::Group { uin: 42 },
                        Reply::with_preview(&text_message(1)).unwrap(),
                        false,
                        cx,
                    );
                    view.submit_attachment(
                        Conversation::Group { uin: 42 },
                        "/tmp/attachment".into(),
                        kind,
                        cx,
                    );
                    assert!(view.reply_jump.is_none());
                    assert!(view.send_scroll.is_some());
                });
            });
            cx.run_until_parked();
            for tick in 0..35 {
                cx.background_executor
                    .advance_clock(Duration::from_millis(16));
                cx.run_until_parked();
                if tick == 3 {
                    cx.update(|cx| assert!(view.read(cx).scroll.offset().y < px(-1.)));
                }
            }
            frames(cx, main, 50);
            cx.update(|cx| {
                let view = view.read(cx);
                assert!((view.scroll.offset().y + view.scroll.max_offset().y).abs() < px(1.));
            });
        }
        let id = std::iter::from_fn(|| commands.try_recv().ok())
            .find_map(|command| {
                if let Command::Attachment { id, .. } = command {
                    Some(id)
                } else {
                    None
                }
            })
            .unwrap();
        cx.update(|cx| view.read(cx).scroll.scroll_to_top_of_item(0));
        frames(cx, main, 8);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Sent {
                    id,
                    result: Err("发送失败".into()),
                });
                view.retry_send(id, "group:42", "attachment", cx);
                assert!(view.send_scroll.is_some());
                assert!(!view.model.failures.contains_key(&id));
            });
        });
        frames(cx, main, 100);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!((view.scroll.offset().y + view.scroll.max_offset().y).abs() < px(1.));
        });
    }

    #[gpui::test]
    fn external_picture_drop_inserts_inline_and_sends_one_ordered_message(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let path = draft_image_file(root.path(), "image.png");
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.composer.update(cx, |input, cx| {
                    input.set_value("前后", window, cx);
                    input.set_selected_range(3..3, cx);
                });
                cx.notify();
            })
        })
        .unwrap();
        drop_files(cx, main, vec![path.clone()]);
        frames(cx, main, 3);
        cx.update(|cx| {
            let view = view.read(cx);
            let input = view.composer.read(cx);
            assert_eq!(input.value().as_ref(), "前\u{fffc}后");
            assert_eq!(input.tokens().len(), 1);
            assert!(input.line_height().unwrap() >= px(72.));
        });
        let bounds = VisualTestContext::from_window(main, cx)
            .debug_bounds("composer-image")
            .unwrap();
        assert_eq!(bounds.size.height, px(72.));
        assert!(
            !std::iter::from_fn(|| commands.try_recv().ok()).any(|command| matches!(
                command,
                Command::Compose { .. } | Command::Attachment { .. } | Command::Send { .. }
            ))
        );
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| view.send(window, cx))
        })
        .unwrap();
        let outgoing = std::iter::from_fn(|| commands.try_recv().ok())
            .filter(|command| {
                matches!(
                    command,
                    Command::Compose { .. } | Command::Attachment { .. } | Command::Send { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(outgoing.len(), 1);
        let Command::Compose { parts, .. } = &outgoing[0] else {
            panic!("mixed message was not composed");
        };
        assert!(
            matches!(parts.as_slice(),[ComposePart::Element(before),ComposePart::Image(image),ComposePart::Element(after)] if matches!(before.as_ref(),MessageElement::Text(text) if text=="前") && image == &path && matches!(after.as_ref(),MessageElement::Text(text) if text=="后"))
        );
        cx.update(|cx| {
            let view = view.read(cx);
            let message = view.model.messages["group:42"].last().unwrap();
            assert!(matches!(
                message.elements.as_slice(),
                [
                    MessageElement::Text(_),
                    MessageElement::Image(_),
                    MessageElement::Text(_)
                ]
            ));
            assert_eq!(view.assets.get(&super::image_key(message, 1)), Some(&path));
            assert!(view.composer.read(cx).tokens().is_empty());
        });
    }
    #[gpui::test]
    fn image_picker_keeps_multiple_inline_images_editable_and_restores_draft(
        cx: &mut TestAppContext,
    ) {
        let root = tempfile::tempdir().unwrap();
        let paths = vec![
            draft_image_file(root.path(), "one.png"),
            draft_image_file(root.path(), "two.png"),
        ];
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.attach(AttachmentKind::Image, window, cx);
            })
        })
        .unwrap();
        cx.simulate_path_prompt_response(|options| {
            assert!(options.multiple);
            Some(paths.clone())
        });
        cx.run_until_parked();
        frames(cx, main, 2);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert_eq!(view.composer.read(cx).tokens().len(), 2);
                view.composer
                    .update(cx, |input, cx| input.replace("末尾", window, cx));
                view.commit_select(Conversation::Group { uin: 99 }, window, cx);
                assert!(view.composer.read(cx).tokens().is_empty());
                view.commit_select(Conversation::Group { uin: 42 }, window, cx);
                assert_eq!(view.composer.read(cx).tokens().len(), 2);
                assert!(view.composer.read(cx).value().ends_with("末尾"));
            })
        })
        .unwrap();
        assert!(
            !std::iter::from_fn(|| commands.try_recv().ok()).any(|command| matches!(
                command,
                Command::Compose { .. } | Command::Attachment { .. } | Command::Send { .. }
            ))
        );
        cx.simulate_keystrokes(main, "home delete");
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).composer.read(cx).tokens().len(), 1));
    }
    #[gpui::test]
    fn dropped_file_and_voice_require_confirmation_and_leave_image_draft_intact(
        cx: &mut TestAppContext,
    ) {
        let root = tempfile::tempdir().unwrap();
        let picture = draft_image_file(root.path(), "image.png");
        let file = root.path().join("document.txt");
        std::fs::write(&file, b"file").unwrap();
        let voice = root.path().join("voice.silk");
        std::fs::write(&voice, b"voice").unwrap();
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        drop_files(cx, main, vec![picture, file.clone(), voice.clone()]);
        draw(cx, main);
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("attachment-confirmation")
                .is_some()
        );
        assert!(
            !std::iter::from_fn(|| commands.try_recv().ok()).any(|command| matches!(
                command,
                Command::Attachment { .. } | Command::Compose { .. }
            ))
        );
        cx.simulate_keystrokes(main, "escape");
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.confirmation.is_none());
            assert_eq!(view.composer.read(cx).tokens().len(), 1);
        });
        drop_files(cx, main, vec![file.clone(), voice.clone()]);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                assert!(
                    view.confirmation.is_some(),
                    "missing confirmation after second drop; status: {}",
                    view.status
                );
                view.confirm_attachments(window, cx)
            })
        })
        .unwrap();
        let outgoing = std::iter::from_fn(|| commands.try_recv().ok())
            .filter(|command| {
                matches!(
                    command,
                    Command::Attachment { .. } | Command::Compose { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(outgoing.len(), 2);
        assert!(
            matches!(&outgoing[0],Command::Attachment {path,kind:AttachmentKind::File,..} if path==&file)
        );
        assert!(
            matches!(&outgoing[1],Command::Attachment {path,kind:AttachmentKind::Voice,..} if path==&voice)
        );
        cx.update(|cx| assert_eq!(view.read(cx).composer.read(cx).tokens().len(), 1));
    }

    fn clipboard_key(cx: &mut TestAppContext, window: AnyWindowHandle, letter: &str) {
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.simulate_keystrokes(window, &format!("{modifier}-{letter}"));
        cx.run_until_parked();
    }
    #[gpui::test]
    fn native_clipboard_image_pastes_inline_without_sending(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let source = draft_image_file(root.path(), "clipboard.png");
        let bytes = std::fs::read(source).unwrap();
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.composer
                    .update(cx, |input, cx| input.focus(window, cx));
                cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    bytes.clone(),
                )));
                cx.notify();
            })
        })
        .unwrap();
        draw(cx, main);
        clipboard_key(cx, main, "v");
        frames(cx, main, 2);
        cx.update(|cx| {
            let view = view.read(cx);
            let input = view.composer.read(cx);
            assert_eq!(input.tokens().len(), 1);
            let path = &view.compose_images[input.tokens()[0].token().id().as_ref()];
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        });
        assert!(
            !std::iter::from_fn(|| commands.try_recv().ok()).any(|command| matches!(
                command,
                Command::Compose { .. } | Command::Send { .. } | Command::Attachment { .. }
            ))
        );
    }
    #[gpui::test]
    fn mixed_draft_copy_cut_and_paste_preserve_text_and_picture_order(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let source = draft_image_file(root.path(), "clipboard.png");
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.composer.update(cx, |input, cx| {
                    input.set_value("前后", window, cx);
                    input.set_selected_range(3..3, cx);
                });
                view.insert_image(source, window, cx);
            })
        })
        .unwrap();
        draw(cx, main);
        clipboard_key(cx, main, "a");
        clipboard_key(cx, main, "c");
        cx.update(|cx| {
            let item = cx.read_from_clipboard().unwrap();
            assert_eq!(item.text().as_deref(), Some("前[图片]后"));
            assert!(
                ferrsphen::rich_clipboard::rich_parts(&item)
                    .unwrap()
                    .is_some()
            );
        });
        clipboard_key(cx, main, "x");
        cx.update(|cx| assert!(view.read(cx).composer.read(cx).value().is_empty()));
        clipboard_key(cx, main, "v");
        cx.update(|cx| {
            let view = view.read(cx);
            let input = view.composer.read(cx);
            assert_eq!(input.value().as_ref(), "前\u{fffc}后");
            assert_eq!(input.tokens().len(), 1);
            assert_eq!(input.tokens()[0].range(), 3..6);
        });
    }
    #[gpui::test]
    fn message_copy_fetches_missing_images_without_opening_a_viewer(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let source = draft_image_file(root.path(), "clipboard.png");
        let (main, view, updates, mut commands) = fixture(cx);
        let mut message = picture_message(1, 32, 24);
        message
            .elements
            .insert(0, MessageElement::Text("前".into()));
        message.elements.push(MessageElement::Text("后".into()));
        let key = super::image_key(&message, 1);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(message.clone(), false);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("old".into()));
                view.copy_message(message.clone(), cx);
            })
        });
        assert!(std::iter::from_fn(|| commands.try_recv().ok()).any(
            |command| matches!(command,Command::ViewImage {key:ref candidate,..} if candidate==&key)
        ));
        updates
            .try_send(Update::Image {
                key: key.clone(),
                result: Ok(source.clone()),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(view.read(cx).viewer_window.is_none());
            let item = cx.read_from_clipboard().unwrap();
            assert_eq!(item.text().as_deref(), Some("前[图片]后"));
            let parts = ferrsphen::rich_clipboard::rich_parts(&item)
                .unwrap()
                .unwrap();
            assert!(matches!(
                parts.as_slice(),
                [
                    ferrsphen::rich_clipboard::Part::Text { .. },
                    ferrsphen::rich_clipboard::Part::Image { .. },
                    ferrsphen::rich_clipboard::Part::Text { .. }
                ]
            ));
        });
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.assets.remove(&key);
                view.copy_message(message, cx);
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("new selection".into()));
            })
        });
        updates
            .try_send(Update::Image {
                key,
                result: Ok(source),
            })
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("new selection")
            )
        });
        let _ = main;
    }
    #[gpui::test]
    fn file_and_voice_pickers_wait_for_the_confirmation_button(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        for (kind, name) in [
            (AttachmentKind::File, "file.txt"),
            (AttachmentKind::Voice, "voice.silk"),
        ] {
            let path = root.path().join(name);
            std::fs::write(&path, b"data").unwrap();
            cx.update_window(main, |_, window, cx| {
                view.update(cx, |view, cx| view.attach(kind, window, cx))
            })
            .unwrap();
            cx.simulate_path_prompt_response(|_| Some(vec![path.clone()]));
            cx.run_until_parked();
            draw(cx, main);
            assert!(
                !std::iter::from_fn(|| commands.try_recv().ok())
                    .any(|command| matches!(command, Command::Attachment { .. }))
            );
            let mut visual = VisualTestContext::from_window(main, cx);
            let button = visual.debug_bounds("attachment-confirm-button").unwrap();
            visual.simulate_click(button.center(), Default::default());
            cx.run_until_parked();
            let outgoing = std::iter::from_fn(|| commands.try_recv().ok())
                .filter(|command| matches!(command, Command::Attachment { .. }))
                .collect::<Vec<_>>();
            assert_eq!(outgoing.len(), 1);
            assert!(
                matches!(&outgoing[0],Command::Attachment {path:sent,kind:sent_kind,..} if sent==&path && sent_kind==&kind)
            );
            cx.update(|cx| assert!(view.read(cx).confirmation.is_none()));
        }
    }

    #[gpui::test]
    fn fullscreen_shortcut_reaches_main_and_detail_windows(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.composer
                    .update(cx, |composer, cx| composer.focus(window, cx));
                cx.notify();
            })
        })
        .unwrap();
        draw(cx, main);
        cx.simulate_keystrokes(main, "f11");
        assert!(
            cx.update_window(main, |_, window, _| window.is_fullscreen())
                .unwrap()
        );
        cx.simulate_keystrokes(main, "f11");
        assert!(
            !cx.update_window(main, |_, window, _| window.is_fullscreen())
                .unwrap()
        );
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.forward = Some(vec![text_message(2)]);
                view.open_detail_window(DetailKind::Forward, cx);
            })
        });
        cx.run_until_parked();
        let forward = cx.update(|cx| view.read(cx).forward_window.unwrap());
        draw(cx, forward);
        cx.simulate_keystrokes(forward, "f11");
        assert!(
            cx.update_window(forward, |_, window, _| window.is_fullscreen())
                .unwrap()
        );
        cx.simulate_keystrokes(forward, "f11");
        assert!(
            !cx.update_window(forward, |_, window, _| window.is_fullscreen())
                .unwrap()
        );
    }
    #[gpui::test]
    fn forward_card_and_window_are_smaller_while_avatars_keep_their_size(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                message.elements = vec![MessageElement::Forward(
                    atri_qq::messages::ForwardReference {
                        resource_id: "record".into(),
                        count: 1,
                        light_app_json: None,
                        raw_element: None,
                    },
                )];
                view.model.receive(message, false);
                view.forward_cache
                    .insert("group:42:1:0".into(), vec![text_message(2)]);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let card = visual.debug_bounds("forward-card:group:42:1:0").unwrap();
        assert_eq!(card.size.width, px(224.));
        visual.simulate_click(card.center(), Default::default());
        cx.run_until_parked();
        let forward = cx.update(|cx| view.read(cx).forward_window.unwrap());
        draw(cx, forward);
        let dimensions = cx
            .update_window(forward, |_, window, _| {
                window.window_bounds().get_bounds().size
            })
            .unwrap();
        assert_eq!(dimensions, size(px(600.), px(720.)));
        let avatar = VisualTestContext::from_window(forward, cx)
            .debug_bounds("avatar:7")
            .unwrap();
        assert_eq!(avatar.size, size(px(36.), px(36.)));
    }
    #[gpui::test]
    fn forwarded_uid_resolves_once_and_shares_the_uin_avatar_cache(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, _| {
                let mut first = text_message(1);
                first.sender_uin = 0;
                first.sender_uid = "forwarded-peer".into();
                let mut second = first.clone();
                second.sequence = 2;
                view.forward = Some(vec![first, second]);
                view.request_assets();
                view.request_assets();
            })
        });
        let resolves = std::iter::from_fn(|| commands.try_recv().ok()).filter(|command| matches!(command, Command::ResolveUser { uid } if uid == "forwarded-peer")).count();
        assert_eq!(resolves, 1);
        cx.update(|cx| {
            view.update(cx, |view, _| {
                view.apply(Update::UserUin {
                    uid: "forwarded-peer".into(),
                    result: Ok(77),
                })
            })
        });
        let avatars = std::iter::from_fn(|| commands.try_recv().ok())
            .filter(|command| matches!(command, Command::Asset { key, .. } if key == "avatar:77"))
            .count();
        assert_eq!(avatars, 1);
        cx.update(|cx| {
            view.update(cx, |view, _| {
                view.apply(Update::Asset {
                    key: "avatar:77".into(),
                    path: "/cached/avatar.png".into(),
                });
                view.request_assets();
                assert_eq!(
                    view.model.sender_uin(&view.forward.as_ref().unwrap()[0]),
                    77
                );
            })
        });
        assert!(commands.try_recv().is_err());
        let _ = main;
    }
    #[gpui::test]
    fn forward_scrolling_fetches_avatars_beyond_the_initial_prefetch(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.forward = Some(
                    (1..=110)
                        .map(|sequence| {
                            let mut message = text_message(sequence);
                            message.sender_uin = sequence + 100;
                            message.sender_uid = format!("peer-{sequence}");
                            message
                        })
                        .collect(),
                );
                view.request_assets();
                view.open_detail_window(DetailKind::Forward, cx);
            })
        });
        cx.run_until_parked();
        while commands.try_recv().is_ok() {}
        let forward = cx.update(|cx| view.read(cx).forward_window.unwrap());
        draw(cx, forward);
        cx.update(|cx| view.update(cx, |view, _| view.forward_scroll.scroll_to_top_of_item(100)));
        draw(cx, forward);
        let position = cx.update(|cx| view.read(cx).forward_scroll.bounds().center());
        wheel(cx, forward, position, -10., TouchPhase::Moved);
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| commands.try_recv().ok()).any(
                |command| matches!(command, Command::Asset { key, .. } if key == "avatar:201")
            )
        );
        let _ = main;
    }
    #[gpui::test]
    fn large_timeline_builds_only_nearby_message_bodies(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=2000 {
                    view.model.receive(text_message(sequence), false);
                }
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        frames(cx, main, 8);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(
                view.lazy_heights.borrow().len() < 100,
                "offscreen bodies were rendered"
            );
            assert!(
                view.bubble_sizes
                    .borrow()
                    .contains_key("group:42:2000:2000")
            );
            view.scroll.scroll_to_top_of_item(0);
        });
        frames(cx, main, 8);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.bubble_sizes.borrow().contains_key("group:42:1:1"));
            assert!(view.lazy_heights.borrow().len() < 150);
        });
    }

    #[gpui::test]
    fn reaching_top_loads_once_and_history_preserves_visible_message(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 31..=70 {
                    view.model.receive(text_message(sequence), false);
                }
                let state = view.model.history.entry("group:42".into()).or_default();
                state.loaded = true;
                state.cursor = Some(Cursor::Group(60));
                cx.notify();
            })
        });
        draw(cx, main);
        let position = cx.update(|cx| view.read(cx).scroll.bounds().center());
        wheel(cx, main, position, 80., TouchPhase::Started);
        assert!(matches!(
            std::iter::from_fn(|| commands.try_recv().ok())
                .find(|command| matches!(command, Command::History { .. }))
                .unwrap(),
            Command::History {
                cursor: Some(Cursor::Group(30)),
                ..
            }
        ));
        wheel(cx, main, position, 20., TouchPhase::Moved);
        assert!(
            commands.try_recv().is_err(),
            "an in-flight page must not be requested twice"
        );
        draw(cx, main);
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("history-loading")
                .is_some()
        );
        let before = cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.bounce.position > 0.);
            view.scroll.bounds_for_item(0).unwrap().top() + view.scroll.offset().y
                - view.scroll.bounds().top()
        });
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::History {
                    key: "group:42".into(),
                    result: Ok(((1..=30).map(text_message).collect(), None)),
                });
                cx.notify();
            })
        });
        frames(cx, main, 10);
        cx.update(|cx| {
            let view = view.read(cx);
            let after = view.scroll.bounds_for_item(30).unwrap().top() + view.scroll.offset().y
                - view.scroll.bounds().top();
            assert!((after - before).abs() < px(1.), "{before:?} -> {after:?}");
            assert_eq!(view.model.messages["group:42"].len(), 70);
        });
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("history-loading")
                .is_none()
        );
        frames(cx, main, 120);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.scroll
                    .set_offset(point(px(0.), -view.scroll.max_offset().y));
                cx.notify();
            })
        });
        draw(cx, main);
        wheel(cx, main, position, -120., TouchPhase::Started);
        cx.update(|cx| assert!(view.read(cx).bounce.position < 0.));
        wheel(cx, main, position, 0., TouchPhase::Ended);
        frames(cx, main, 120);
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.bounce.position, 0.);
            assert!((view.scroll.offset().y + view.scroll.max_offset().y).abs() < px(1.));
        });
    }
    #[gpui::test]
    fn short_finished_timeline_bounces_without_requesting_history(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.model
                    .history
                    .entry("group:42".into())
                    .or_default()
                    .loaded = true;
                cx.notify();
            })
        });
        draw(cx, main);
        let position = cx.update(|cx| view.read(cx).scroll.bounds().center());
        wheel(cx, main, position, 100., TouchPhase::Started);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.bounce.position > 0.);
            assert_eq!(view.scroll.offset().y, px(0.));
        });
        assert!(commands.try_recv().is_err());
        wheel(cx, main, position, 0., TouchPhase::Ended);
        frames(cx, main, 90);
        cx.update(|cx| assert_eq!(view.read(cx).bounce.position, 0.));
    }
    #[gpui::test]
    fn bounce_settles_without_mouse_events_or_frame_callbacks(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.model
                    .history
                    .entry("group:42".into())
                    .or_default()
                    .loaded = true;
                cx.notify();
            })
        });
        draw(cx, main);
        let position = cx.update(|cx| view.read(cx).scroll.bounds().center());
        wheel(cx, main, position, 120., TouchPhase::Started);
        let pulled = cx.update(|cx| view.read(cx).bounce.position);
        assert!(pulled > 0.);
        cx.run_until_parked();
        // No Ended event, draw calls, or simulated animation frames.
        for tick in 0..90 {
            cx.background_executor
                .advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            if tick == 20 {
                let position = cx.update(|cx| view.read(cx).bounce.position);
                assert!(position >= 0. && position < pulled / 2.);
            }
        }
        cx.update(|cx| {
            assert_eq!(view.read(cx).bounce.position, 0.);
            assert!(!view.read(cx).bounce_animating);
        });
    }

    #[gpui::test]
    fn large_overscroll_ignores_momentum_after_finger_release(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                view.model
                    .history
                    .entry("group:42".into())
                    .or_default()
                    .loaded = true;
                cx.notify();
            })
        });
        draw(cx, main);
        let position = cx.update(|cx| view.read(cx).scroll.bounds().center());
        wheel(cx, main, position, 100_000., TouchPhase::Started);
        wheel(cx, main, position, 0., TouchPhase::Ended);
        cx.run_until_parked();
        for tick in 1..=120 {
            cx.background_executor
                .advance_clock(Duration::from_millis(16));
            wheel(cx, main, position, 500., TouchPhase::Moved);
            cx.run_until_parked();
            if tick >= 45 {
                cx.update(|cx| {
                    assert_eq!(view.read(cx).bounce.position, 0.);
                    assert_eq!(view.read(cx).scroll.offset().y, px(0.));
                });
            }
        }
        wheel(cx, main, position, 80., TouchPhase::Started);
        cx.update(|cx| assert!(view.read(cx).bounce.position > 0.));
    }

    #[gpui::test]
    fn icon_navigation_returns_to_the_selected_chat(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let settings = visual.debug_bounds("nav-settings").unwrap();
        visual.simulate_click(settings.center(), Default::default());
        draw(cx, main);
        assert!(visual.debug_bounds("settings-page").is_some());
        assert!(visual.debug_bounds("conversation-panel").is_none());
        assert!(visual.debug_bounds("group-sidebar").is_none());
        cx.update(|cx| {
            assert!(view.read(cx).page == Page::Settings);
            assert_eq!(view.read(cx).model.selected.as_deref(), Some("group:42"));
        });
        let chat = visual.debug_bounds("nav-chat").unwrap();
        visual.simulate_click(chat.center(), Default::default());
        draw(cx, main);
        assert!(visual.debug_bounds("settings-page").is_none());
        assert!(visual.debug_bounds("conversation-panel").is_some());
        assert!(visual.debug_bounds("group-sidebar").is_some());
    }

    #[gpui::test]
    fn conversation_switch_finishes_exit_before_entering_new_chat(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                let mut other = text_message(2);
                other.conversation = Conversation::Group { uin: 43 };
                view.model.receive(other, false);
                cx.notify();
            })
        });
        draw(cx, main);
        let original_sidebar = VisualTestContext::from_window(main, cx)
            .debug_bounds("group-sidebar")
            .unwrap();
        let original = VisualTestContext::from_window(main, cx)
            .debug_bounds("conversation-panel")
            .unwrap();
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.select(Conversation::Group { uin: 43 }, window, cx)
            });
        })
        .unwrap();
        frames(cx, main, 4);
        cx.update(|cx| assert_eq!(view.read(cx).model.selected.as_deref(), Some("group:42")));
        let shrinking = VisualTestContext::from_window(main, cx)
            .debug_bounds("conversation-panel")
            .unwrap();
        assert!(shrinking.size.width < original.size.width);
        let sidebar = VisualTestContext::from_window(main, cx)
            .debug_bounds("group-sidebar")
            .unwrap();
        assert!(sidebar.size.height < original_sidebar.size.height);
        assert!(sidebar.right() < original_sidebar.right());
        assert!(sidebar.top() > original_sidebar.top());
        frames(cx, main, 6);
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.model.selected.as_deref(), Some("group:43"));
            assert!(view.transition.as_ref().unwrap().entering);
        });
        frames(cx, main, 30);
        cx.update(|cx| assert!(view.read(cx).transition.is_none()));
        let finished = VisualTestContext::from_window(main, cx)
            .debug_bounds("conversation-panel")
            .unwrap();
        assert!((finished.size.width - original.size.width).abs() < px(1.));
    }
    #[gpui::test]
    fn reply_jump_springs_to_message_after_loading_targeted_history(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 2..=40 {
                    view.model.receive(text_message(sequence), false);
                }
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(cx, main);
        let initial = cx.update(|cx| view.read(cx).scroll.offset().y);
        assert!(initial < px(-500.));
        let reply = Reply::with_preview(&text_message(1)).unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.jump_to_reply(Conversation::Group { uin: 42 }, reply.clone(), false, cx)
            })
        });
        assert!(matches!(
            commands.try_recv().unwrap(),
            Command::LocateReply { .. }
        ));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::ReplyHistory {
                    key: "group:42".into(),
                    reply,
                    result: Ok(vec![text_message(1)]),
                });
                cx.notify();
            })
        });
        frames(cx, main, 6);
        let moving = cx.update(|cx| view.read(cx).scroll.offset().y);
        assert!(
            moving > initial && moving < px(-1.),
            "{initial:?} -> {moving:?}"
        );
        frames(cx, main, 120);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.reply_jump.is_none());
            assert!(view.scroll.offset().y.abs() < px(1.));
            assert_eq!(view.model.messages["group:42"].len(), 40);
        });
    }
    #[gpui::test]
    fn separate_picture_messages_expand_before_hover_and_reply_individually(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(picture_message(1, 160, 90), false);
                view.model.receive(picture_message(2, 80, 160), false);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let first = visual.debug_bounds("stack-card:group:42:1:1:0").unwrap();
        visual.simulate_mouse_move(first.center(), None, Default::default());
        cx.update(|cx| assert!(view.read(cx).fades.is_empty()));
        assert!(
            visual
                .debug_bounds("message-actions:group:42:1:1")
                .is_none()
        );
        visual.simulate_mouse_down(first.center(), gpui::MouseButton::Right, Default::default());
        visual.simulate_mouse_up(first.center(), gpui::MouseButton::Right, Default::default());
        cx.update(|cx| assert!(view.read(cx).expanded_images.contains("group:42:1:1")));
        frames(cx, main, 70);
        let first = visual.debug_bounds("stack-card:group:42:1:1:0").unwrap();
        let second = visual.debug_bounds("stack-card:group:42:1:1:1").unwrap();
        assert!(
            (first.size.width - px(160.)).abs() < px(1.),
            "first: {first:?}, second: {second:?}"
        );
        assert!((second.size.width - px(80.)).abs() < px(1.));
        visual.simulate_mouse_move(second.center(), None, Default::default());
        frames(cx, main, 8);
        cx.update(|cx| assert_eq!(view.read(cx).fades["group:42:2:2"].to, 1.));
        let actions = visual.debug_bounds("message-actions:group:42:2:2").unwrap();
        visual.simulate_mouse_move(
            actions.origin + point(px(10.), px(10.)),
            None,
            Default::default(),
        );
        visual.simulate_click(actions.origin + point(px(10.), px(10.)), Default::default());
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.reply.as_ref().unwrap().sequence, 2);
            assert_eq!(view.reply.as_ref().unwrap().elements.len(), 1);
        });
        // The other image's context menu must keep its own source identity.
        let press = first.center();
        visual.simulate_mouse_down(press, gpui::MouseButton::Right, Default::default());
        visual.simulate_mouse_up(press, gpui::MouseButton::Right, Default::default());
        draw(cx, main);
        let item = visual.debug_bounds("message-menu-reply").unwrap().center();
        visual.simulate_mouse_move(item, None, Default::default());
        visual.simulate_click(item, Default::default());
        cx.update(|cx| assert_eq!(view.read(cx).reply.as_ref().unwrap().sequence, 1));
    }
    #[gpui::test]
    fn a_multi_picture_message_keeps_one_reply_bar_below_all_images(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = picture_message(1, 160, 90);
                message
                    .elements
                    .extend(picture_message(2, 80, 160).elements);
                view.model.receive(message, false);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let first = visual.debug_bounds("stack-card:group:42:1:1:0").unwrap();
        visual.simulate_mouse_move(first.center(), None, Default::default());
        frames(cx, main, 8);
        cx.update(|cx| assert_eq!(view.read(cx).fades["group:42:1:1"].to, 1.));
        assert!(
            visual
                .debug_bounds("message-actions:group:42:1:1")
                .is_some()
        );
        visual.simulate_click(first.center(), Default::default());
        frames(cx, main, 70);
        let second = visual.debug_bounds("stack-card:group:42:1:1:1").unwrap();
        visual.simulate_mouse_move(second.center(), None, Default::default());
        frames(cx, main, 8);
        let actions = visual.debug_bounds("message-actions:group:42:1:1").unwrap();
        assert!(actions.top() >= second.bottom());
        assert!(
            visual
                .debug_bounds("message-actions:group:42:2:2")
                .is_none()
        );
        visual.simulate_mouse_move(
            actions.origin + point(px(10.), px(10.)),
            None,
            Default::default(),
        );
        visual.simulate_click(actions.origin + point(px(10.), px(10.)), Default::default());
        cx.update(|cx| assert_eq!(view.read(cx).reply.as_ref().unwrap().elements.len(), 2));
    }
    #[gpui::test]
    fn small_own_pictures_keep_individual_reply_and_copy_controls_accessible(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        let root = tempfile::tempdir().unwrap();
        let image = draft_image_file(root.path(), "copy.png");
        let expected = std::fs::read(&image).unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=2 {
                    let mut message = picture_message(sequence, 32, 24);
                    message.is_self = true;
                    view.assets
                        .insert(super::image_key(&message, 0), image.clone());
                    view.model.receive(message, false);
                }
                view.change_stack("group:42:self:1", true, cx);
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let picture = visual.debug_bounds("stack-card:group:42:self:1:1").unwrap();
        assert!((picture.size.width - px(32.)).abs() < px(1.));
        visual.simulate_mouse_move(picture.center(), None, Default::default());
        frames(cx, main, 8);
        let actions = visual
            .debug_bounds("message-actions:group:42:self:2")
            .unwrap();
        visual.simulate_mouse_move(
            actions.origin + point(px(10.), px(10.)),
            None,
            Default::default(),
        );
        visual.simulate_click(actions.origin + point(px(10.), px(10.)), Default::default());
        cx.update(|cx| assert_eq!(view.read(cx).reply.as_ref().unwrap().sequence, 2));
        let actions = visual
            .debug_bounds("message-actions:group:42:self:2")
            .unwrap();
        visual.simulate_mouse_move(
            actions.origin + point(px(34.), px(10.)),
            None,
            Default::default(),
        );
        visual.simulate_click(actions.origin + point(px(34.), px(10.)), Default::default());
        cx.update(|cx| {
            let item = cx.read_from_clipboard().unwrap();
            let [gpui::ClipboardEntry::Image(image)] = item.entries() else {
                panic!("expected native image clipboard");
            };
            assert_eq!(image.bytes, expected);
        });
    }
    #[gpui::test]
    fn prepending_pictures_keeps_expanded_messages_at_their_reading_position(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 6..=10 {
                    view.model.receive(picture_message(sequence, 80, 90), false);
                }
                view.change_stack("group:42:6:6", true, cx);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let before = visual.debug_bounds("stack-card:group:42:6:6:0").unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.loading_more.insert("group:42".into());
                view.apply(Update::History {
                    key: "group:42".into(),
                    result: Ok((
                        (1..=5)
                            .map(|sequence| picture_message(sequence, 80, 90))
                            .collect(),
                        None,
                    )),
                });
                cx.notify();
            })
        });
        frames(cx, main, 5);
        let after = visual.debug_bounds("stack-card:group:42:1:1:5").unwrap();
        assert!(
            (after.top() - before.top()).abs() < px(1.),
            "{before:?} -> {after:?}"
        );
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.stack_expanded(&view.model.messages["group:42"]));
            assert_eq!(view.model.messages["group:42"].len(), 10);
        });
    }
    #[gpui::test]
    fn card_rotation_and_position_pass_through_intermediate_geometry(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.png");
        let sheet = directory.path().join("sheet.png");
        image::RgbaImage::from_pixel(80, 160, image::Rgba([20, 80, 180, 255]))
            .save(&source)
            .unwrap();
        ferrsphen::card_preview::render(&source, &sheet, 1, 80, 160).unwrap();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(picture_message(1, 160, 90), false);
                let second = picture_message(2, 80, 160);
                let source_key = super::image_key(&second, 0);
                view.assets.insert(source_key.clone(), source);
                view.assets
                    .insert(super::card_key(&source_key, 1, 80, 160), sheet);
                view.model.receive(second, false);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let before = visual.debug_bounds("stack-card:group:42:1:1:1").unwrap();
        cx.update(|cx| view.update(cx, |view, cx| view.change_stack("group:42:1:1", true, cx)));
        frames(cx, main, 5);
        let during = visual.debug_bounds("stack-card:group:42:1:1:1").unwrap();
        frames(cx, main, 70);
        let after = visual.debug_bounds("stack-card:group:42:1:1:1").unwrap();
        let aspect = |bounds: gpui::Bounds<gpui::Pixels>| {
            f32::from(bounds.size.width) / f32::from(bounds.size.height)
        };
        assert!(aspect(before) > aspect(during) && aspect(during) > aspect(after));
        assert!(before.top() < during.top() && during.top() < after.top());
        assert!((after.size.width - px(80.)).abs() < px(1.));
    }
    #[gpui::test]
    fn consecutive_images_stack_but_rich_messages_remain_separate(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let image = MessageElement::Image(MediaReference::new(
                    MediaKind::Image,
                    MediaConversation::Group { uin: 42 },
                ));
                for sequence in 1..=2 {
                    let mut message = text_message(sequence);
                    message.elements = vec![image.clone()];
                    view.model.receive(message, false);
                }
                let mut rich = text_message(3);
                rich.elements.extend([image.clone(), image]);
                view.model.receive(rich, false);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        assert!(visual.debug_bounds("image-stack:group:42:1:1").is_some());
        assert!(visual.debug_bounds("message-bubble:group:42:2:2").is_none());
        assert!(visual.debug_bounds("message-bubble:group:42:3:3").is_some());
        assert!(visual.debug_bounds("image-stack:group:42:3:3").is_none());
    }
    #[gpui::test]
    fn reply_to_image_expands_group_and_scrolls_to_its_position(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=35 {
                    let mut message = text_message(sequence);
                    if sequence <= 5 {
                        message.elements = vec![MessageElement::Image(MediaReference::new(
                            MediaKind::Image,
                            MediaConversation::Group { uin: 42 },
                        ))];
                    }
                    view.model.receive(message, false);
                }
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(cx, main);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                let reply = Reply::from(&view.model.messages["group:42"][3]);
                view.jump_to_reply(Conversation::Group { uin: 42 }, reply, false, cx);
            })
        });
        frames(cx, main, 150);
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.reply_jump.is_none());
            assert!(view.expanded_images.contains("group:42:1:1"));
            let target = view.scroll.bounds().top() + view.scroll.bounds().size.height * 0.3
                - view.scroll.bounds_for_item(0).unwrap().top()
                - px(3. * (188. + ferrsphen::card_preview::ACTION_HEIGHT));
            assert!((view.scroll.offset().y - target).abs() < px(1.));
            assert_eq!(view.model.messages["group:42"].len(), 35);
        });
    }
    #[gpui::test]
    fn video_surface_outside_play_icon_opens_player_and_requests_preview_once(
        cx: &mut TestAppContext,
    ) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                let mut video =
                    MediaReference::new(MediaKind::Video, MediaConversation::Group { uin: 42 });
                video.width = 320;
                video.height = 180;
                video.duration = 90;
                message.recipient_uin = 100;
                message.elements = vec![MessageElement::Video(video)];
                view.model.receive(message, false);
                view.request_assets();
                view.request_assets();
                cx.notify();
            })
        });
        let previews = std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|command| {
                if let Command::VideoPreview { key, reference } = command {
                    assert_eq!(reference.duration, 90);
                    Some(key)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(previews, ["preview:video:group:42:1:0"]);
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let preview = visual.debug_bounds("video:group:42:1:0").unwrap();
        visual.simulate_click(preview.origin + point(px(12.), px(12.)), Default::default());
        cx.run_until_parked();
        assert!(
            matches!(commands.try_recv().unwrap(), Command::ViewVideo { key, reference } if key == "video:group:42:1:0" && reference.duration == 90)
        );
        cx.update(|cx| assert_eq!(cx.windows().len(), 2));
    }
    #[gpui::test]
    fn clicking_voice_requests_internal_playback_and_can_cancel(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                let mut voice =
                    MediaReference::new(MediaKind::Voice, MediaConversation::Group { uin: 42 });
                voice.duration = 3;
                message.elements = vec![MessageElement::Voice(voice)];
                view.model.receive(message, false);
                cx.notify();
            })
        });
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let voice = visual.debug_bounds("voice:group:42:1:0").unwrap();
        visual.simulate_click(voice.center(), Default::default());
        assert!(
            matches!(commands.try_recv().unwrap(), Command::Voice { key, .. } if key == "voice:group:42:1:0")
        );
        draw(cx, main);
        VisualTestContext::from_window(main, cx).simulate_click(voice.center(), Default::default());
        cx.update(|cx| {
            assert!(view.read(cx).voice_player.read(cx).key.is_none());
            assert_eq!(cx.windows().len(), 1);
        });
        assert!(commands.try_recv().is_err());
    }

    #[gpui::test]
    fn video_window_is_deferred_reused_and_displays_download_failure(cx: &mut TestAppContext) {
        let (main, view, _updates, mut commands) = fixture(cx);
        let reference = MediaReference::new(MediaKind::Video, MediaConversation::Group { uin: 42 });
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.open_video("video-test".into(), reference.clone(), cx)
            })
        });
        cx.run_until_parked();
        let player = cx.update(|cx| view.read(cx).video_windows["video-test"]);
        assert_ne!(main.window_id(), player.window_id());
        assert!(
            matches!(commands.try_recv().unwrap(), Command::ViewVideo { key, .. } if key == "video-test")
        );
        draw(cx, player);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Video {
                    key: "video-test".into(),
                    result: Err("下载失败".into()),
                });
                cx.notify();
            })
        });
        cx.run_until_parked();
        draw(cx, player);
        assert!(
            VisualTestContext::from_window(player, cx)
                .debug_bounds("video-error")
                .is_some()
        );
        cx.update(|cx| assert_eq!(cx.windows().len(), 2));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.open_video("video-test".into(), reference, cx)
            })
        });
        cx.run_until_parked();
        assert!(commands.try_recv().is_err());
        cx.update(|cx| {
            assert_eq!(
                view.read(cx).video_windows["video-test"].window_id(),
                player.window_id()
            )
        });
    }

    #[gpui::test]
    fn detail_windows_open_after_owner_update_and_can_be_reused(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        let reference = MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 42 });
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_image("image".into(), reference.clone(), window, cx);
                assert!(
                    view.viewer_window.is_none(),
                    "window creation must wait for the entity lease to end"
                );
            });
        })
        .unwrap();
        cx.run_until_parked();
        let image = cx.update(|cx| view.read(cx).viewer_window.unwrap());
        assert_eq!(cx.windows().len(), 2);
        draw(cx, image);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.open_image("image".into(), reference, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 2);
        draw(cx, image);
        cx.update_window(image, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        cx.update(|cx| {
            assert!(view.read(cx).viewer_window.is_none());
            assert!(view.read(cx).viewer.is_none());
        });
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                let reference =
                    MediaReference::new(MediaKind::Image, MediaConversation::Group { uin: 42 });
                view.open_image("image".into(), reference, window, cx);
            });
        })
        .unwrap();
        cx.run_until_parked();
        let reopened = cx.update(|cx| view.read(cx).viewer_window.unwrap());
        assert_ne!(image.window_id(), reopened.window_id());
        assert_eq!(cx.windows().len(), 2);
        draw(cx, reopened);
        cx.update_window(reopened, |_, window, cx| {
            view.update(cx, |view, cx| view.close_image(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.forward = Some(Vec::new());
                view.open_detail_window(DetailKind::Forward, cx);
                assert!(view.forward_window.is_none());
            })
        });
        cx.run_until_parked();
        let forward = cx.update(|cx| view.read(cx).forward_window.unwrap());
        assert_eq!(cx.windows().len(), 2);
        draw(cx, forward);
        cx.update_window(forward, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.forward_window.is_none());
            assert!(view.forward.is_none());
        });
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.open_video(
                    "closing-video".into(),
                    MediaReference::new(MediaKind::Video, MediaConversation::Group { uin: 42 }),
                    cx,
                );
            })
        });
        cx.run_until_parked();
        let video = cx.update(|cx| view.read(cx).video_windows["closing-video"]);
        cx.update_window(video, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                assert!(view.video_windows.is_empty());
                view.video_results
                    .insert("closing-video".into(), Err("late download".into()));
                cx.notify();
            })
        });
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
    }
    #[gpui::test]
    fn visible_recall_animates_once_and_navigation_does_not_replay_it(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                message.elements = vec![MessageElement::Text("long message ".repeat(30))];
                view.model.receive(message, false);
                cx.notify();
            })
        });
        draw(cx, main);
        let target = cx
            .update_window(main, |_, window, cx| recalled_size(window, cx))
            .unwrap();
        let original = VisualTestContext::from_window(main, cx)
            .debug_bounds("message-bubble:group:42:1:1")
            .unwrap();
        assert!(original.size.width > target.width + px(50.));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                assert!(view.visible_bubbles.borrow().contains_key("group:42:1:1"));
                view.apply(Update::Recalled {
                    conversation: Conversation::Group { uin: 42 },
                    sequence: 1,
                });
                assert_eq!(view.recall_tweens.len(), 1);
                cx.notify();
            })
        });
        draw(cx, main);
        frames(cx, main, 5);
        let intermediate = VisualTestContext::from_window(main, cx)
            .debug_bounds("message-bubble:group:42:1:1")
            .unwrap();
        assert!(
            intermediate.size.width > target.width + px(1.)
                && intermediate.size.width < original.size.width
        );
        cx.update(|cx| {
            view.update(cx, |view, _| {
                let started = view.recall_tweens["group:42:1:1"].started;
                view.apply(Update::Notification(atri_qq::messages::PushEvent::Recall {
                    conversation: Conversation::Group { uin: 42 },
                    sequence: 1,
                    random: None,
                    time: None,
                    author_uid: "peer".into(),
                    operator_uid: None,
                    tip: String::new(),
                }));
                assert_eq!(view.recall_tweens["group:42:1:1"].started, started);
            })
        });
        let mut visual = VisualTestContext::from_window(main, cx);
        let settings = visual.debug_bounds("nav-settings").unwrap();
        visual.simulate_click(settings.center(), Default::default());
        draw(cx, main);
        let mut visual = VisualTestContext::from_window(main, cx);
        let chat = visual.debug_bounds("nav-chat").unwrap();
        visual.simulate_click(chat.center(), Default::default());
        draw(cx, main);
        let restored = VisualTestContext::from_window(main, cx)
            .debug_bounds("message-bubble:group:42:1:1")
            .unwrap();
        assert!((restored.size.width - target.width).abs() < px(1.));
        assert!((restored.size.height - target.height).abs() < px(1.));
        cx.update(|cx| assert!(view.read(cx).recall_tweens.is_empty()));
        frames(cx, main, 3);
        let restored = VisualTestContext::from_window(main, cx)
            .debug_bounds("message-bubble:group:42:1:1")
            .unwrap();
        assert!((restored.size.width - target.width).abs() < px(1.));
    }
    #[gpui::test]
    fn background_and_offscreen_recalls_never_arm_a_tween(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=35 {
                    view.model.receive(text_message(sequence), false);
                }
                let mut other = text_message(100);
                other.conversation = Conversation::Group { uin: 43 };
                view.model.receive(other, false);
                view.scroll.scroll_to_bottom();
                cx.notify();
            })
        });
        draw(cx, main);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                assert!(!view.visible_bubbles.borrow().contains_key("group:42:1:1"));
                view.apply(Update::Recalled {
                    conversation: Conversation::Group { uin: 42 },
                    sequence: 1,
                });
                view.apply(Update::Recalled {
                    conversation: Conversation::Group { uin: 43 },
                    sequence: 100,
                });
                assert!(view.recall_tweens.is_empty());
                cx.notify();
            })
        });
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.commit_select(Conversation::Group { uin: 43 }, window, cx)
            })
        })
        .unwrap();
        draw(cx, main);
        let target = cx
            .update_window(main, |_, window, cx| recalled_size(window, cx))
            .unwrap();
        let actual = VisualTestContext::from_window(main, cx)
            .debug_bounds("message-bubble:group:43:100:100")
            .unwrap();
        assert!((actual.size.width - target.width).abs() < px(1.));
        cx.update(|cx| assert!(view.read(cx).recall_tweens.is_empty()));
    }
    #[gpui::test]
    fn completed_recall_tween_is_consumed_without_input(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                view.model.receive(text_message(1), false);
                cx.notify();
            })
        });
        draw(cx, main);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Recalled {
                    conversation: Conversation::Group { uin: 42 },
                    sequence: 1,
                });
                cx.notify();
            })
        });
        draw(cx, main);
        cx.background_executor.advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(view.read(cx).recall_tweens.is_empty());
            assert!(!view.read(cx).recall_animating);
        });
    }
    #[gpui::test]
    fn private_recall_push_updates_a_legacy_cached_bubble(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                message.conversation = Conversation::Friend {
                    uin: 7,
                    uid: "peer".into(),
                };
                message.sequence = 444;
                message.client_sequence = 333;
                message.raw = vec![
                    0x0a, 2, 8, 7, 0x12, 9, 8, 0xa6, 1, 0x28, 0xbc, 3, 0x58, 0xcd, 2,
                ];
                view.model.receive(message, false);
                view.apply(Update::Notification(atri_qq::messages::PushEvent::Recall {
                    conversation: Conversation::Friend {
                        uin: 0,
                        uid: "peer".into(),
                    },
                    sequence: 333,
                    random: Some(666),
                    time: None,
                    author_uid: "peer".into(),
                    operator_uid: Some("peer".into()),
                    tip: String::new(),
                }));
                assert!(
                    view.model
                        .is_recalled(&view.model.messages["dm:uid:peer"][0])
                );
                cx.notify();
            })
        });
        frames(cx, main, 120);
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("recall-label:dm:uid:peer:333:1")
                .is_some()
        );
    }
    #[gpui::test]
    fn server_history_tombstone_renders_recall_label(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let mut message = text_message(1);
                message.elements.clear();
                message.raw = vec![0x0a, 6, 8, 7, 0x42, 2, 8, 42, 0x12, 4, 8, 82, 0x28, 1];
                view.apply(Update::History {
                    key: "group:42".into(),
                    result: Ok((vec![message], None)),
                });
                cx.notify();
            })
        });
        frames(cx, main, 120);
        assert!(
            VisualTestContext::from_window(main, cx)
                .debug_bounds("recall-label:group:42:1:1")
                .is_some()
        );
        cx.update(|cx| assert!(view.read(cx).model.recalled.contains("group:42:1")));
    }
    #[gpui::test]
    fn recalled_bubble_settles_to_label_width_not_original_message_width(cx: &mut TestAppContext) {
        let (main, view, _updates, _commands) = fixture(cx);
        cx.update_window(main, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                let message = Message {
                    conversation: Conversation::Group { uin: 42 },
                    sender_uin: 7,
                    sender_uid: "peer".into(),
                    recipient_uin: 42,
                    recipient_uid: String::new(),
                    sender_name: "Peer".into(),
                    sequence: 1,
                    client_sequence: 1,
                    message_id: 1,
                    random: 0,
                    time: 1700000000,
                    is_self: false,
                    is_bot: None,
                    elements: vec![MessageElement::Text("long message ".repeat(30))],
                    metadata: vec![],
                    raw: vec![],
                };
                view.model.receive(message, false);
                view.model.recalled.insert("group:42:1".into());
                view.bubble_sizes
                    .borrow_mut()
                    .insert("group:42:1:1".into(), size(px(400.), px(200.)));
                assert!(recalled_size(window, cx).width < px(112.));
                cx.notify();
            });
        })
        .unwrap();
        for _ in 0..120 {
            cx.background_executor
                .advance_clock(Duration::from_millis(16));
            cx.update_window(main, |_, window, cx| {
                window.simulate_next_frame(cx);
                window.draw(cx).clear(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
        let target = cx
            .update_window(main, |_, window, cx| recalled_size(window, cx))
            .unwrap();
        let mut visual = VisualTestContext::from_window(main, cx);
        let actual = visual
            .debug_bounds("message-bubble:group:42:1:1")
            .expect("recalled bubble");
        assert!((actual.size.width - target.width).abs() < px(1.));
        assert!((actual.size.height - target.height).abs() < px(1.));
    }
}

fn phone_layout(viewport: gpui_kit::Size<Pixels>) -> bool {
    viewport.width.min(viewport.height) < px(600.)
}

fn face_image(face: &crate::expressions::Face, assets: &HashMap<String, PathBuf>) -> AnyElement {
    let path = assets.get(&format!("face:{}", face.id));
    div()
        .min_w(px(24.))
        .h(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .child(if let Some(path) = path {
            img(path.clone())
                .size(px(24.))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            div().text_sm().child(face.text()).into_any_element()
        })
        .into_any_element()
}
