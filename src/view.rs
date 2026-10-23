use atri_qq::{
    contacts::GroupMember,
    messages::{Conversation, Mention, Message, MessageElement, PushEvent, Reply},
};
use ferrsphen::ui_motion::{CardTextureCache, card_frame};
use ferrsphen::{
    backend::{AttachmentKind, Command, Update},
    model::{Model, conversation_key, image_only, message_groups, reply_target, summary},
    ui_motion::{Bounce, ENTRANCE_SPAN, ENTRANCE_ZOOM, EXIT_SPAN, TimelineViewport, page_progress},
    viewer::{ImageItem, Viewer},
};
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        spinner::Spinner,
        *,
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Chat,
    Settings,
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
    commands: mpsc::Sender<Command>,
    account: Entity<InputState>,
    password: Entity<InputState>,
    search: Entity<InputState>,
    search_index: RefCell<ferrsphen::search::SearchIndex>,
    composer: Entity<TextareaState>,
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
                    .when(self.sidebar && self.is_group(), |panel| {
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
            "[已撤回]".to_string()
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
                "消息".into()
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
                            .child("图片")
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
            self.forward_error = Some("未找到原消息".into());
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
                self.forward_error = Some("未找到原消息".into());
            } else {
                self.error = Some("未找到原消息".into());
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
        let account = cx.new(|cx| InputState::new(window, cx).placeholder("QQ 账号"));
        let password = cx.new(|cx| InputState::new(window, cx).placeholder("密码").masked(true));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入消息")
                .submit_on_enter(true)
        });
        let ticket = cx.new(|cx| InputState::new(window, cx).placeholder("ticket"));
        let captcha_random = cx.new(|cx| InputState::new(window, cx).placeholder("randstr"));
        let voice_player = ferrsphen::audio::VoicePlayer::new(cx);
        let subscriptions = vec![
            cx.observe(&voice_player, |_, _, cx| cx.notify()),
            cx.subscribe_in(&search, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&password, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.login(window, cx);
                }
            }),
            cx.subscribe_in(
                &composer,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Change => {
                        if let Some(key) = this.model.selected.clone()
                            && let Some(chat) = this
                                .model
                                .chats
                                .iter_mut()
                                .find(|chat| conversation_key(&chat.conversation) == key)
                        {
                            chat.draft = input.read(cx).value().to_string();
                        }
                        this.persist();
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];
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
        Self {
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
            page: Page::Chat,
            commands,
            account,
            password,
            search,
            search_index: RefCell::default(),
            composer,
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
            _subscriptions: subscriptions,
            _updates: task,
        }
    }
    fn command(&mut self, command: Command) -> bool {
        if self.commands.try_send(command).is_err() {
            self.error = Some("操作队列不可用".into());
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
        if matches!(
            &update,
            Update::Connecting | Update::LoginRequired | Update::Challenge(_)
        ) {
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
                    self.transition = None;
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
                self.recall_tweens.clear();
                self.visible_bubbles.borrow_mut().clear();
                self.scroll_anchor = None;
                self.reply_jump = None;
                self.bounce = Bounce::default();
                self.loading_more.clear();
                let profile = self.model.profile.take();
                self.model = *model;
                self.model.profile = profile;
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
                self.model.receive(message, true);
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
            Update::Notification(event) => {
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
        self.request_assets();
        if near_bottom
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
                    .filter_map(move |(index, element)| {
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
            self.error = Some("请输入 QQ 账号".into());
            cx.notify();
            return;
        };
        let password = self.password.read(cx).value().to_string();
        if password.is_empty() {
            self.error = Some("请输入密码".into());
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
        if self.model.selected.as_ref() == Some(&conversation_key(&conversation)) {
            self.transition = None;
            cx.notify();
            return;
        }
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
        self.bounce = Bounce::default();
        self.scroll_anchor = None;
        self.recall_tweens.clear();
        self.visible_bubbles.borrow_mut().clear();
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
        self.composer.update(cx, |input, cx| {
            input.set_value(draft, window, cx);
            input.focus(window, cx);
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
        if self.status == "离线"
            || self
                .transition
                .as_ref()
                .is_some_and(|transition| !transition.entering)
        {
            return;
        }
        let Some(chat) = self.model.selected_chat() else {
            return;
        };
        let conversation = chat.conversation.clone();
        let mut text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let mut elements = Vec::new();
        for mention in self.mentions.drain(..) {
            text = text.replacen(&format!("@{} ", mention.display), "", 1);
            elements.push(MessageElement::Mention(mention));
        }
        if let Some(reply) = self.reply.as_ref() {
            match Reply::with_preview(reply) {
                Ok(reply) => elements.push(MessageElement::Reply(reply)),
                Err(error) => {
                    self.error = Some(error.to_string());
                    cx.notify();
                    return;
                }
            }
        }
        self.reply = None;
        self.reply_jump = None;
        self.scroll_anchor = None;
        elements.push(MessageElement::Text(text.clone()));
        self.sequence += 1;
        let id = self.sequence;
        let nonce = (rand::random::<u32>() & 0x7fff_ffff).max(1);
        let optimistic_elements = elements.clone();
        let command = Command::Send {
            id,
            nonce,
            conversation: conversation.clone(),
            elements,
        };
        if self.command(command.clone()) {
            self.model.stage(
                id,
                conversation.clone(),
                optimistic_elements,
                nonce,
                chrono::Utc::now().timestamp(),
            );
            self.scroll.scroll_to_bottom();
            self.retries.insert(id, command);
            self.model
                .pending
                .insert(id, (conversation_key(&conversation), text));
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.persist();
        }
        cx.notify();
    }
    fn attach(&mut self, kind: AttachmentKind, cx: &mut Context<Self>) {
        let Some(chat) = self.model.selected_chat() else {
            return;
        };
        let conversation = chat.conversation.clone();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择文件".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    this.sequence += 1;
                    let id = this.sequence;
                    let text = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let nonce = (rand::random::<u32>() & 0x7fff_ffff).max(1);
                    let local_path = path.clone();
                    let command = Command::Attachment {
                        id,
                        nonce,
                        conversation: conversation.clone(),
                        path,
                        kind,
                    };
                    if this.command(command.clone()) {
                        let media_kind = match kind {
                            AttachmentKind::Image => atri_qq::media::MediaKind::Image,
                            AttachmentKind::Voice => atri_qq::media::MediaKind::Voice,
                            AttachmentKind::Video => atri_qq::media::MediaKind::Video,
                            AttachmentKind::File => atri_qq::media::MediaKind::File,
                        };
                        let mut reference = atri_qq::media::MediaReference::new(
                            media_kind,
                            conversation.media_conversation().expect("chat media"),
                        );
                        reference.name = text.clone();
                        let element = match kind {
                            AttachmentKind::Image => MessageElement::Image(reference),
                            AttachmentKind::Voice => MessageElement::Voice(reference),
                            AttachmentKind::Video => MessageElement::Video(reference),
                            AttachmentKind::File => MessageElement::File(reference),
                        };
                        let staged = this.model.stage(
                            id,
                            conversation.clone(),
                            vec![element],
                            nonce,
                            chrono::Utc::now().timestamp(),
                        );
                        if matches!(kind, AttachmentKind::Image) {
                            this.assets.insert(image_key(&staged, 0), local_path);
                        }
                        this.scroll.scroll_to_bottom();
                        this.retries.insert(id, command);
                        this.model
                            .pending
                            .insert(id, (conversation_key(&conversation), text));
                    }
                    cx.notify();
                });
            }
        })
        .detach();
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
                .child("正在登录…")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_8()
            .p_8()
            .child(
                div()
                    .flex()
                    .gap_10()
                    .items_center()
                    .child(
                        div()
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
                                        Some(image) => {
                                            element.child(img(image.clone()).size_full())
                                        }
                                        None => element.child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("二维码加载中"),
                                        ),
                                    }),
                            )
                            .child(
                                Button::new("refresh-qr")
                                    .ghost()
                                    .label("刷新二维码")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.qr = None;
                                        this.command(Command::Qr);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_64()
                            .gap_4()
                            .child(div().text_lg().child("登录 QQ"))
                            .child(Input::new(&self.account).aria_label("QQ 账号"))
                            .child(Input::new(&self.password).aria_label("密码"))
                            .child(
                                Button::new("login")
                                    .primary()
                                    .w_full()
                                    .label("登录")
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.login(window, cx)),
                                    ),
                            ),
                    ),
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
                    .label("打开验证")
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
                        .label("提交验证")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let ticket = this.ticket.read(cx).value().to_string();
                            let random = this.captcha_random.read(cx).value().to_string();
                            if ticket.is_empty() || random.is_empty() {
                                this.error = Some("请填写验证结果".into());
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
                        .accessibility_label("群详情")
                        .tooltip("群详情")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sidebar = !this.sidebar;
                            cx.notify();
                        })),
                )
            })
            .child(div().w_3())
            .into_any_element()
    }
    fn navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::assets::IconName;
        div()
            .debug_selector(|| "navigation".into())
            .flex()
            .flex_col()
            .items_center()
            .w(px(52.))
            .h_full()
            .flex_shrink_0()
            .py_3()
            .gap_2()
            .border_r_1()
            .border_color(cx.theme().border)
            .children(
                [
                    (Page::Chat, "nav-chat", "聊天", IconName::MessageCircle),
                    (Page::Settings, "nav-settings", "设置", IconName::Settings),
                ]
                .into_iter()
                .map(|(page, id, label, icon)| {
                    div().debug_selector(move || id.into()).child(
                        Button::new(id)
                            .ghost()
                            .icon(icon)
                            .tooltip(label)
                            .accessibility_label(label)
                            .when(self.page == page, |button| button.bg(cx.theme().accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.page != page {
                                    this.recall_tweens.clear();
                                    this.visible_bubbles.borrow_mut().clear();
                                }
                                this.page = page;
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
            .flex()
            .flex_col()
            .w_72()
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .p_3()
                    .child(Input::new(&self.search).aria_label("搜索会话")),
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
            .ghost()
            .selected(selected)
            .when(selected, |button| button.bg(cx.theme().accent))
            .w_full()
            .h(px(72.))
            .p_3()
            .accessibility_label(format!("打开会话 {}", chat.title))
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
            MessageElement::Text(text) => div().text_sm().child(text.clone()).into_any_element(),
            MessageElement::Mention(mention) => div()
                .text_sm()
                .text_color(cx.theme().primary)
                .child(format!("@{}", mention.display))
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
                    .accessibility_label("打开图片")
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
                            .child("图片")
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
                                "重试".into()
                            } else if active && position > 0. {
                                format!("{} / {} 秒", position as u32, duration.ceil() as u32)
                            } else {
                                format!("{} 秒", reference.duration)
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
                                player.complete(key.clone(), Err("操作队列不可用".into()), cx)
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
                    .label(format!("文件 · {}", reference.name))
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
                    .map(|rows| format!("查看 {} 条消息", rows.len()))
                    .unwrap_or_else(|| "查看聊天记录".into());
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
                                    .child("聊天记录"),
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
                    .unwrap_or_else(|| "卡片".into());
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
                    .label(format!("群文件 · {}", file.name))
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
                let text = format!("字段 {}\n{}", value.field, hex::encode(&value.payload));
                Button::new(SharedString::from(format!("opaque:{key}")))
                    .ghost()
                    .label("其他消息")
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
            let text = if recalled {
                "[已撤回]".into()
            } else {
                summary(&message.elements)
            };
            let reply_owner = owner.clone();
            let recall_owner = owner.clone();
            menu.item(
                PopupMenuItem::new("回复")
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
            .item(PopupMenuItem::new("复制").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
            }))
            .item(
                PopupMenuItem::element(move |_, cx| {
                    div()
                        .text_color(cx.theme().foreground.opacity(if recallable {
                            1.
                        } else {
                            0.4
                        }))
                        .child("撤回")
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
                    PopupMenuItem::new("查看本地文件")
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
        let expanded_width = sizes
            .iter()
            .map(|size| size.width)
            .fold(if separate_messages { 112. } else { 0. }, f32::max);
        let action_height = if separate_messages { ACTION_HEIGHT } else { 0. };
        let expanded_height = sizes
            .iter()
            .map(|size| size.height + action_height + 8.)
            .sum::<f32>();
        let collapsed_width = collapsed.width;
        let collapsed_height = collapsed.height;
        let mut top = 0.;
        let cards = images
            .iter()
            .enumerate()
            .map(|(order, (message, index, reference))| {
                let size = sizes[order];
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
                let actions = (separate_messages && expanded)
                    .then(|| self.message_actions(message, cx, read_only, true));
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
                                rotated.height + if interactive { ACTION_HEIGHT + 16. } else { 0. }
                            ))
                            .when(interactive, |card| card.p_2().m(px(-8.)))
                            .when(order >= visible_cards && progress <= 0.001, |card| {
                                card.invisible()
                            })
                            .when(order >= visible_cards, |card| card.opacity(progress))
                            .child(trigger)
                            .when_some(actions, |card, actions| {
                                card.child(div().h(px(ACTION_HEIGHT)).child(actions))
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
                        .tooltip("收起图片")
                        .accessibility_label("收起图片")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_stack(&collapse_key, false, cx);
                        })),
                )
            })
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
        let text = summary(&message.elements);
        let actions = div()
            .flex()
            .gap_1()
            .child(
                Button::new(SharedString::from(format!("reply:{key}")))
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Reply)
                    .disabled(recalled || read_only)
                    .tooltip("回复")
                    .accessibility_label("回复")
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
                    .tooltip("复制")
                    .accessibility_label("复制")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(if recalled {
                            "[已撤回]".into()
                        } else {
                            text.clone()
                        }))
                    }),
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
                    .child("[已撤回]")
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
            .overflow_hidden()
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
            .child(self.avatar(
                self.model.sender_uin(message),
                &self.model.sender_name(message),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .max_w(rems(36.))
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
                            .when(!separate_messages, |col| col.child(actions)),
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
        let anchor = if !read_only && self.scroll_anchor.is_none() && initialized {
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
                        .child(self.message_row(messages, window, cx, read_only))
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
        let rows = self.lazy_message_rows(
            self.model
                .messages
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            window,
            cx,
            false,
        );
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
                    .child(format!("{text} · 发送中"))
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
                    .child(format!("发送失败 · {error}"))
                    .child(
                        Button::new(SharedString::from(format!("retry:{id}")))
                            .ghost()
                            .small()
                            .label("重试")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(command) = this.retries.get(&id).cloned()
                                    && this.command(command)
                                {
                                    this.model
                                        .pending
                                        .insert(id, (retry_key.clone(), text.clone()));
                                    this.model.failures.remove(&id);
                                }
                                cx.notify();
                            })),
                    )
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h_12()
                    .px_5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
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
                            .px_5()
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
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .p_3()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                Button::new("image-upload")
                                    .ghost()
                                    .small()
                                    .label("图片")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.attach(AttachmentKind::Image, cx)
                                    })),
                            )
                            .child(
                                Button::new("file-upload")
                                    .ghost()
                                    .small()
                                    .label("文件")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.attach(AttachmentKind::File, cx)
                                    })),
                            )
                            .child(
                                Button::new("voice-upload")
                                    .ghost()
                                    .small()
                                    .label("语音")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.attach(AttachmentKind::Voice, cx)
                                    })),
                            )
                            .child(
                                Button::new("video-upload")
                                    .ghost()
                                    .small()
                                    .label("视频")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.attach(AttachmentKind::Video, cx)
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
                                        .label("取消")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.reply = None;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .h_24()
                            .bordered(false)
                            .aria_label("消息内容"),
                    )
                    .child(
                        div().flex().justify_end().child(
                            Button::new("send")
                                .primary()
                                .label("发送")
                                .disabled(
                                    self.composer.read(cx).value().trim().is_empty()
                                        || self.status == "离线",
                                )
                                .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
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
                            .child("暂无公告"),
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
                .child("加载失败")
                .into_any_element(),
            None => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("正在加载…")
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
                                    .child("群公告"),
                            )
                            .child(
                                Button::new("refresh-announcements")
                                    .ghost()
                                    .xsmall()
                                    .icon(gpui_kit::assets::IconName::RefreshCw)
                                    .accessibility_label("刷新群公告")
                                    .tooltip("刷新")
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
                    .child(format!("群成员 {}", members.map_or(0, Vec::len))),
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
        let uid = member.uid.clone();
        let uin = member.uin;
        let display = name.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .py_2()
            .child(self.avatar(member.uin, &name, cx))
            .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
            .child(
                Button::new(SharedString::from(format!("mention:{uin}:{uid}")))
                    .ghost()
                    .xsmall()
                    .label("@")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.mentions.push(Mention {
                            uin,
                            uid: Some(uid.clone()),
                            display: display.clone(),
                            is_everyone: false,
                        });
                        this.composer.update(cx, |input, cx| {
                            let text = format!("{}@{} ", input.value(), display);
                            input.set_value(text, window, cx);
                            input.focus(window, cx);
                        });
                        cx.notify();
                    })),
            )
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
                            .insert(key.clone(), Err("操作队列不可用".into()));
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
                    ..ferrsphen::window_chrome::options("视频")
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
                DetailKind::Image => "图片",
                DetailKind::Forward => "聊天记录",
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
                ferrsphen::window_chrome::title_bar(cx)
                    .child(div().flex_1().text_center().text_sm().child("聊天记录")),
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
                                    .child("正在加载…"),
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
                            PopupMenuItem::new("查看本地文件")
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
                    "加载失败"
                } else {
                    "正在加载…"
                })
                .when(viewer.error.is_some(), |view| {
                    view.child(Button::new("retry-image").ghost().label("重试").on_click(
                        cx.listener(|this, _, _, cx| {
                            if let Some(viewer) = &mut this.viewer {
                                viewer.error = None;
                            }
                            this.request_viewer_image();
                            cx.notify();
                        }),
                    ))
                })
                .into_any_element(),
        };
        let name = if item.reference.name.is_empty() {
            "图片".into()
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
                    .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .when(cfg!(target_os = "macos"), |bar| {
                                bar.child(
                                    Button::new("viewer-close")
                                        .ghost()
                                        .icon(gpui_kit::assets::IconName::X)
                                        .tooltip("关闭")
                                        .accessibility_label("关闭图片")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_image(window, cx)
                                        })),
                                )
                            }),
                    ),
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
                            .tooltip("上一张")
                            .accessibility_label("上一张")
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
                            .tooltip("下一张")
                            .accessibility_label("下一张")
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
                            .tooltip("缩小")
                            .accessibility_label("缩小")
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
                            .tooltip("适应窗口")
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
                            .tooltip("放大")
                            .accessibility_label("放大")
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
                            .child(div().flex_1().text_sm().child("消息详情"))
                            .child(
                                Button::new("close-details")
                                    .ghost()
                                    .small()
                                    .label("关闭")
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
            .child(if matches!(self.phase, Phase::Chat) {
                self.topbar(window, cx)
            } else {
                ferrsphen::window_chrome::title_bar(cx)
                    .child(div().flex_1())
                    .into_any_element()
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
                                .label("关闭")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .map(|view| {
                if matches!(self.phase, Phase::Chat) {
                    view.child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.navigation(cx))
                            .map(|body| {
                                if self.page == Page::Chat {
                                    body.child(self.chat_list(cx))
                                        .child(self.conversation_panel(window, cx))
                                } else {
                                    body.child(
                                        div()
                                            .debug_selector(|| "settings-page".into())
                                            .flex_1()
                                            .h_full(),
                                    )
                                }
                            }),
                    )
                } else {
                    view.child(self.login_view(cx))
                        .when(self.challenge.is_some(), |view| {
                            view.child(self.challenge_view(cx))
                        })
                }
            })
            .when(self.details.is_some(), |view| view.child(self.overlay(cx)));
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
    let label: SharedString = "[已撤回]".into();
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
    use ferrsphen::backend::{Command, Update};
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
                |window, cx| cx.new(|cx| AppView::new(commands, receiver, window, cx)),
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
        let item = press + point(px(30.), px(17.));
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
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.phase = Phase::Chat;
                for sequence in 1..=2 {
                    let mut message = picture_message(sequence, 32, 24);
                    message.is_self = true;
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
            assert_eq!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .as_deref(),
                Some("[图片]")
            )
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
                message.elements = vec![MessageElement::Video(video)];
                view.model.receive(message, false);
                view.request_assets();
                view.request_assets();
                cx.notify();
            })
        });
        let previews = std::iter::from_fn(|| commands.try_recv().ok())
            .filter_map(|command| {
                if let Command::VideoPreview { key, .. } = command {
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
            matches!(commands.try_recv().unwrap(), Command::ViewVideo { key, .. } if key == "video:group:42:1:0")
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
        cx.update_window(image, |_, window, cx| {
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
