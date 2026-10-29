use crate::adapters::{Backend, Command, Update};
use crate::{
    chat_ui::{Fade, Page},
    settings::Preferences,
    settings_ui::Section as SettingsSection,
};
use chat_adapter::{
    AccountId, Action, AuthInput, AuthState, ChatId, Content, Conversation, Descriptor,
    DisplayPart, Event, ExtensionContent, ExtensionRequest, Field, FieldKind, Layout, Media,
    Message, MessageId, PageRequest, Scope, SendOutcome, SendRequest, Timeline, User,
};
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        spinner::Spinner,
        *,
    },
    prelude::FluentBuilder,
    *,
};
use rust_i18n::t;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{Duration, Instant},
};
struct Form {
    step: String,
    fields: Vec<(Field, Entity<InputState>)>,
}
struct ActionForm {
    action: Action,
    request: ExtensionRequest,
    fields: Vec<(Field, Entity<InputState>)>,
}
struct ConversationTransition {
    destination: ChatId,
    started: Instant,
    entering: bool,
}
pub struct AdapterView {
    backend: Backend,
    protocol_switch: Option<crate::launcher::ProtocolSwitch>,
    session_hooks: Option<crate::accounts::SessionHooks>,
    descriptor: Option<Descriptor>,
    login: chat_adapter::LoginSnapshot,
    page: Page,
    phone: bool,
    mobile_platform: bool,
    phone_conversation: bool,
    transition: Option<ConversationTransition>,
    settings_section: SettingsSection,
    preferences: Preferences,
    applied_locale: String,
    fades: BTreeMap<MessageId, Fade>,
    form: Option<Form>,
    qr: Option<(String, std::sync::Arc<Image>)>,
    action: Option<ActionForm>,
    account: Option<AccountId>,
    profile: Option<User>,
    chats: Vec<Conversation>,
    selected: Option<ChatId>,
    timeline: Timeline,
    rows: Vec<MessageId>,
    list: ListState,
    history: BTreeMap<ChatId, Option<String>>,
    loading: BTreeSet<ChatId>,
    chat_cursor: Option<String>,
    composer: Entity<TextareaState>,
    drafts: BTreeMap<ChatId, String>,
    search: Entity<InputState>,
    search_index: std::cell::RefCell<crate::search::SearchIndex>,
    reply: Option<MessageId>,
    media: BTreeMap<String, PathBuf>,
    downloads: BTreeSet<String>,
    failed_downloads: BTreeSet<String>,
    error: Option<String>,
    pending: BTreeSet<String>,
    arrivals: BTreeMap<MessageId, Instant>,
    window: AnyWindowHandle,
    main_window: bool,
    _updates: Task<()>,
    _subscriptions: Vec<Subscription>,
}
fn qr_image(url: &str) -> anyhow::Result<std::sync::Arc<Image>> {
    let code = qrcode::QrCode::new(url.as_bytes())?;
    let side = code.width();
    let scale = 6;
    let quiet = 4;
    let bitmap = image::GrayImage::from_fn(
        ((side + quiet * 2) * scale) as u32,
        ((side + quiet * 2) * scale) as u32,
        |x, y| {
            let x = x as usize / scale;
            let y = y as usize / scale;
            let dark = x >= quiet
                && y >= quiet
                && x < side + quiet
                && y < side + quiet
                && code[(x - quiet, y - quiet)] == qrcode::Color::Dark;
            image::Luma([if dark { 0 } else { 255 }])
        },
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    bitmap.write_to(&mut bytes, image::ImageFormat::Png)?;
    Ok(std::sync::Arc::new(Image::from_bytes(
        ImageFormat::Png,
        bytes.into_inner(),
    )))
}
fn choice_label(choice: &chat_adapter::LoginChoice) -> SharedString {
    match choice.id.as_str() {
        "qr" => t!("login.qr_login").into_owned(),
        "phone" | "phone_login" => t!("adapter.phone_login").into_owned(),
        "password" => t!("adapter.password").into_owned(),
        "sso" => t!("adapter.sso").into_owned(),
        "token" => t!("adapter.token").into_owned(),
        _ => choice.label.clone(),
    }
    .into()
}
fn field_label(field: &Field) -> SharedString {
    match field.id.as_str() {
        "api_id" => "API ID".into(),
        "api_hash" => "API Hash".into(),
        "phone" => t!("adapter.phone").into_owned().into(),
        "code" => t!("adapter.code").into_owned().into(),
        "password" => t!("adapter.password").into_owned().into(),
        "homeserver" => t!("adapter.homeserver").into_owned().into(),
        "username" => t!("adapter.username").into_owned().into(),
        "recovery_key" => t!("adapter.recovery_key").into_owned().into(),
        "token" => t!("adapter.token").into_owned().into(),
        _ => field.label.clone().into(),
    }
}
fn inputs(
    fields: Vec<Field>,
    window: &mut Window,
    cx: &mut Context<AdapterView>,
) -> Vec<(Field, Entity<InputState>)> {
    fields
        .into_iter()
        .map(|field| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(field_label(&field))
                    .masked(field.kind == FieldKind::Secret)
            });
            (field, input)
        })
        .collect()
}
impl AdapterView {
    pub fn new(mut backend: Backend, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut updates = backend.updates.take().expect("adapter updates");
        let task = cx.spawn(async move |this, cx| {
            while let Some(update) = updates.recv().await {
                let handle = match this.update(cx, |this, _| this.window) {
                    Ok(handle) => handle,
                    Err(_) => break,
                };
                if handle
                    .update(cx, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            this.apply(update, window, cx);
                            cx.notify();
                        })
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(t!("chat.input"))
                .auto_grow(1, 6)
                .submit_on_enter(!crate::platform::is_mobile())
        });
        let search =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("chat.search_conversations")));
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        let owner = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            if event.is_scrolled && event.visible_range.start <= 1 {
                let owner = owner.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |this, cx| this.load_earlier(cx));
                });
            }
        });
        let appearance_owner = cx.entity().downgrade();
        let subscriptions = vec![
            window.observe_window_appearance(move |window, cx| {
                let appearance = window.appearance();
                let owner = appearance_owner.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if this.preferences.appearance == crate::settings::Appearance::System {
                            crate::settings::apply_theme(
                                this.preferences.appearance,
                                appearance,
                                cx,
                            );
                            cx.refresh_windows();
                        }
                    });
                });
            }),
            cx.subscribe_in(
                &composer,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                    InputEvent::Change => {
                        if let Some(chat) = &this.selected {
                            this.drafts
                                .insert(chat.clone(), input.read(cx).value().to_string());
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&search, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
        ];
        Self {
            backend,
            protocol_switch: None,
            session_hooks: None,
            descriptor: None,
            login: Default::default(),
            page: Page::Chat,
            phone: false,
            mobile_platform: crate::platform::is_mobile(),
            phone_conversation: false,
            transition: None,
            settings_section: SettingsSection::General,
            preferences: Preferences::default(),
            applied_locale: rust_i18n::locale().to_string(),
            fades: BTreeMap::new(),
            form: None,
            qr: None,
            action: None,
            account: None,
            profile: None,
            chats: vec![],
            selected: None,
            timeline: Timeline::default(),
            rows: vec![],
            list,
            history: BTreeMap::new(),
            loading: BTreeSet::new(),
            chat_cursor: None,
            composer,
            drafts: BTreeMap::new(),
            search,
            search_index: Default::default(),
            reply: None,
            media: BTreeMap::new(),
            downloads: BTreeSet::new(),
            failed_downloads: BTreeSet::new(),
            error: None,
            pending: BTreeSet::new(),
            arrivals: BTreeMap::new(),
            window: window.window_handle(),
            main_window: false,
            _updates: task,
            _subscriptions: subscriptions,
        }
    }
    pub fn with_session_hooks(mut self, hooks: crate::accounts::SessionHooks) -> Self {
        self.protocol_switch = Some(hooks.accounts.clone());
        self.session_hooks = Some(hooks);
        self.main_window = true;
        self
    }
    pub fn with_protocol_switch(mut self, switch: crate::launcher::ProtocolSwitch) -> Self {
        self.protocol_switch = Some(switch);
        self
    }
    fn command(&mut self, command: Command) {
        if self.backend.commands.send(command).is_err() {
            self.error = Some(t!("error.queue").into_owned());
        }
    }
    fn refresh_rows(&mut self) {
        let rows: Vec<_> = self
            .selected
            .as_ref()
            .map(|chat| {
                self.timeline
                    .in_chat(chat)
                    .into_iter()
                    .map(|message| message.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let old = self.rows.len();
        if self.rows != rows {
            let prefix = self
                .rows
                .iter()
                .zip(&rows)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = self.rows[prefix..]
                .iter()
                .rev()
                .zip(rows[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            self.list
                .splice(prefix..old - suffix, rows.len() - prefix - suffix);
            self.rows = rows;
        }
    }
    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::AuthResult { generation, result } => {
                if generation == self.login.generation {
                    match result {
                        Ok(auth) => self.apply(Update::Auth(auth), window, cx),
                        Err(error) => {
                            self.login.busy = false;
                            self.error = Some(error.to_string());
                        }
                    }
                }
            }
            Update::Login(snapshot) => {
                let error = snapshot.error.as_ref().map(ToString::to_string);
                let hooks = self.session_hooks.take();
                self.apply(Update::Auth(snapshot.state.clone()), window, cx);
                self.session_hooks = hooks;
                self.login = snapshot;
                self.error = error;
                if let Some(hooks) = &self.session_hooks {
                    (hooks.on_login)(self.login.clone(), cx);
                }
            }
            Update::Descriptor(descriptor) => self.descriptor = Some(descriptor),
            Update::Preferences(preferences) => {
                self.preferences = preferences;
                crate::settings::apply(preferences, cx);
            }
            Update::Auth(auth) => {
                self.login.busy = false;
                if let AuthState::Qr { url, .. } = &auth {
                    if self.qr.as_ref().is_none_or(|(current, _)| current != url) {
                        match qr_image(url) {
                            Ok(image) => self.qr = Some((url.clone(), image)),
                            Err(error) => {
                                self.qr = None;
                                self.error = Some(error.to_string());
                            }
                        }
                    }
                } else {
                    self.qr = None;
                }

                if let AuthState::Form { step, fields } = &auth {
                    if !self.form.as_ref().is_some_and(|form| form.step == *step) {
                        self.form = Some(Form {
                            step: step.clone(),
                            fields: inputs(fields.clone(), window, cx),
                        });
                    }
                } else {
                    self.form = None;
                }
                if let AuthState::Ready { account, user } = &auth {
                    self.account = Some(account.clone());
                    self.profile = Some(user.clone());
                }
                self.login.state = auth;
                if let Some(hooks) = &self.session_hooks {
                    (hooks.on_login)(self.login.clone(), cx);
                }
            }
            Update::Conversations { page, append } => {
                if !append {
                    self.chats.clear();
                }
                for chat in page.items {
                    self.upsert_chat(chat);
                }
                self.chat_cursor = page.next;
            }
            Update::History { chat, page, cached } => {
                for message in page.items {
                    self.timeline.apply(&Event::Message(message));
                }
                if !cached {
                    self.loading.remove(&chat);
                    self.history.insert(chat, page.next);
                }
                self.refresh_rows();
            }
            Update::Event(Event::Message(message)) => {
                self.arrivals
                    .entry(message.id.clone())
                    .or_insert_with(|| cx.background_executor().now());
                if let Some(chat) = self
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == message.id.chat)
                {
                    chat.last_activity_ms = chat.last_activity_ms.max(message.time_ms);
                }
                self.timeline.apply(&Event::Message(message));
                self.refresh_rows();
            }
            Update::Event(Event::Conversation(chat)) => self.upsert_chat(chat),
            Update::Event(Event::Deleted(ids)) => {
                self.timeline.apply(&Event::Deleted(ids));
            }
            Update::Event(Event::ResyncRequired) => {
                self.command(Command::Conversations(PageRequest::default()));
                if let Some(chat) = self.selected.clone() {
                    self.fetch_history(chat, None);
                }
            }
            Update::Event(Event::Error(error)) | Update::Error(error) => {
                self.login.busy = false;
                self.loading.clear();
                self.error = Some(error.to_string());
            }
            Update::Event(Event::Auth(auth)) => self.apply(Update::Auth(auth), window, cx),
            Update::Sent { client_id, result } => {
                self.pending.remove(&client_id);
                match result {
                    Ok(SendOutcome::Confirmed(message)) => {
                        self.timeline.apply(&Event::Message(*message));
                        self.refresh_rows();
                    }
                    Ok(SendOutcome::Accepted { .. }) => {}
                    Err(error) => {
                        self.error = Some(error.to_string());
                    }
                }
            }
            Update::Downloaded { id, result } => {
                self.downloads.remove(&id);
                match result {
                    Ok(path) => {
                        self.media.insert(id, path);
                    }
                    Err(error) => {
                        self.failed_downloads.insert(id);
                        self.error = Some(error.to_string());
                    }
                }
            }
        }
        cx.notify();
    }
    fn upsert_chat(&mut self, chat: Conversation) {
        if let Some(old) = self.chats.iter_mut().find(|old| old.id == chat.id) {
            *old = chat;
        } else {
            self.chats.push(chat);
        }
    }
    fn fetch_history(&mut self, chat: ChatId, cursor: Option<String>) {
        if self.loading.insert(chat.clone()) {
            self.command(Command::History(chat, PageRequest { cursor, limit: 50 }));
        }
    }
    fn load_earlier(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.selected.clone()
            && let Some(Some(cursor)) = self.history.get(&chat).cloned()
        {
            self.fetch_history(chat, Some(cursor));
            cx.notify();
        }
    }
    fn select(&mut self, chat: ChatId, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&chat) {
            self.page = Page::Chat;
            self.phone_conversation = true;
            cx.notify();
            return;
        }
        if self.selected.is_some() && !self.phone && !cx.reduce_motion() {
            self.transition = Some(ConversationTransition {
                destination: chat,
                started: cx.background_executor().now(),
                entering: false,
            });
            cx.notify();
            return;
        }
        self.commit_select(chat, window, cx);
    }
    fn commit_select(&mut self, chat: ChatId, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&chat) {
            return;
        }
        self.selected = Some(chat.clone());
        self.page = Page::Chat;
        self.phone_conversation = true;
        self.fades.clear();
        self.reply = None;
        self.arrivals.clear();
        self.list.reset(0);
        self.rows.clear();
        let draft = self.drafts.get(&chat).cloned().unwrap_or_default();
        self.composer
            .update(cx, |input, cx| input.set_value(draft, window, cx));
        self.refresh_rows();
        self.list.scroll_to_end();
        self.fetch_history(chat, None);
        cx.notify();
    }
    fn advance_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut transition) = self.transition.take() else {
            return;
        };
        let now = cx.background_executor().now();
        let span = if transition.entering {
            crate::ui_motion::ENTRANCE_SPAN
        } else {
            crate::ui_motion::EXIT_SPAN
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
    fn conversation_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let visible = self.transition.as_ref().map_or(1., |transition| {
            let span = if transition.entering {
                crate::ui_motion::ENTRANCE_SPAN
            } else {
                crate::ui_motion::EXIT_SPAN
            };
            let progress = (cx
                .background_executor()
                .now()
                .saturating_duration_since(transition.started)
                .as_secs_f32()
                / span.as_secs_f32())
            .clamp(0., 1.);
            if transition.entering {
                crate::ui_motion::page_progress(progress)
            } else {
                1. - crate::ui_motion::page_progress(progress)
            }
        });
        let scale = 1. - crate::ui_motion::ENTRANCE_ZOOM * (1. - visible);
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
                    .child(self.conversation_view(self.phone, cx))
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
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let client_id = (rand::random::<i64>() & i64::MAX).max(1).to_string();
        let sender = self.profile.clone().unwrap_or(User {
            id: String::new(),
            name: String::new(),
            avatar: None,
        });
        let quoted = self.reply.as_ref().map(|id| {
            Box::new(chat_adapter::Reply {
                target: id.clone(),
                sender: self
                    .timeline
                    .messages
                    .get(id)
                    .map(|message| message.sender.clone())
                    .unwrap_or(User {
                        id: String::new(),
                        name: String::new(),
                        avatar: None,
                    }),
                preview: self
                    .timeline
                    .messages
                    .get(id)
                    .map(|message| message.content.clone())
                    .unwrap_or_default(),
            })
        });
        let message = Message {
            id: MessageId {
                chat: chat.clone(),
                id: format!("pending:{client_id}"),
            },
            client_id: Some(client_id.clone()),
            sender,
            time_ms: chrono::Utc::now().timestamp_millis(),
            outgoing: true,
            content: vec![Content::Text(text.clone())],
            reply: quoted,
            deleted: false,
        };
        self.timeline.apply(&Event::Message(message.clone()));
        self.pending.insert(client_id.clone());
        self.arrivals.insert(message.id, Instant::now());
        let reply_to = self.reply.take();
        self.command(Command::Send(SendRequest {
            chat: chat.clone(),
            client_id,
            content: vec![Content::Text(text)],
            reply_to,
        }));
        self.composer
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.drafts.remove(&chat);
        self.refresh_rows();
        self.list.scroll_to_end();
        cx.notify();
    }
    fn submit_auth(&mut self, cx: &mut Context<Self>) {
        if self.login.busy {
            return;
        }
        let Some(form) = &self.form else {
            return;
        };
        let fields: BTreeMap<_, _> = form
            .fields
            .iter()
            .map(|(field, input)| (field.id.clone(), input.read(cx).value().to_string()))
            .collect();
        if form
            .fields
            .iter()
            .any(|(field, _)| field.required && fields[&field.id].trim().is_empty())
        {
            return;
        }
        let step = form.step.clone();
        self.login.busy = true;
        self.error = None;
        self.command(Command::Authenticate(AuthInput { step, fields }));
        cx.notify();
    }
    fn open_action(
        &mut self,
        action: Action,
        request: ExtensionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.fields.is_empty() {
            self.command(Command::Execute(request));
        } else {
            self.action = Some(ActionForm {
                fields: inputs(action.fields.clone(), window, cx),
                action,
                request,
            });
        }
        cx.notify();
    }
    fn submit_action(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.action else {
            return;
        };
        let fields: serde_json::Map<_, _> = form
            .fields
            .iter()
            .map(|(field, input)| {
                (
                    field.id.clone(),
                    serde_json::Value::String(input.read(cx).value().to_string()),
                )
            })
            .collect();
        if form.fields.iter().any(|(field, _)| {
            field.required && fields[&field.id].as_str().unwrap().trim().is_empty()
        }) {
            return;
        }
        let mut request = form.request.clone();
        request.data = fields.into();
        self.command(Command::Execute(request));
        self.action = None;
        cx.notify();
    }
    fn avatar_view(
        &mut self,
        media: Option<&Media>,
        name: &str,
        side: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(media) = media {
            if let Some(path) = self.media.get(&media.id) {
                return img(path.clone())
                    .w(px(side))
                    .h(px(side))
                    .rounded_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element();
            }
            if !self.failed_downloads.contains(&media.id) && self.downloads.insert(media.id.clone())
            {
                self.command(Command::Download(media.clone()));
            }
        }
        crate::chat_ui::avatar(px(side), cx)
            .text_color(cx.theme().muted_foreground)
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .child(name.chars().next().map(String::from).unwrap_or_default())
            .into_any_element()
    }
    fn media_view(&mut self, media: &Media, cx: &mut Context<Self>) -> AnyElement {
        if let Some(path) = self.media.get(&media.id) {
            return img(path.clone())
                .w(px(200.))
                .max_h(px(260.))
                .rounded_lg()
                .object_fit(ObjectFit::Contain)
                .into_any_element();
        }
        if self.failed_downloads.contains(&media.id) {
            return div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t!("error.image_unavailable"))
                .into_any_element();
        }
        if self.downloads.insert(media.id.clone()) {
            self.command(Command::Download(media.clone()));
        }
        div()
            .w(px(160.))
            .h(px(120.))
            .rounded_lg()
            .bg(cx.theme().muted)
            .flex()
            .items_center()
            .justify_center()
            .child(Spinner::new())
            .into_any_element()
    }
    fn extension_view(
        &mut self,
        extension: &ExtensionContent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .debug_selector(|| "adapter-extension".into())
            .flex()
            .flex_wrap()
            .gap_1()
            .children(extension.display.iter().map(|part| {
                match part {
                    DisplayPart::Text(text) => div()
                        .child(crate::message_text::MessageText::new(
                            "extension-text",
                            text.clone(),
                        ))
                        .into_any_element(),
                    DisplayPart::User(user) => div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(user.name.clone())
                        .into_any_element(),
                    DisplayPart::Image(media) => self.media_view(media, cx),
                }
            }))
            .into_any_element()
    }
    fn set_message_hover(&mut self, id: &MessageId, hovered: bool, cx: &mut Context<Self>) {
        let to = if hovered { 1. } else { 0. };
        let old = self.fades.get(id);
        if old.is_some_and(|fade| fade.to == to) {
            return;
        }
        let from = old.map_or(0., Fade::value);
        let generation = old.map_or(1, |fade| fade.generation + 1);
        self.fades.insert(
            id.clone(),
            Fade {
                from,
                to,
                started: Instant::now(),
                generation,
            },
        );
        cx.notify();
    }
    fn reply_preview(&mut self, reply: &chat_adapter::Reply, cx: &mut Context<Self>) -> AnyElement {
        let target = self.timeline.messages.get(&reply.target).cloned();
        let sender = target
            .as_ref()
            .map_or(&reply.sender, |message| &message.sender);
        let sender = sender.name.clone();
        let content = target
            .as_ref()
            .map_or(&reply.preview, |message| &message.content);
        let text = content
            .iter()
            .map(Content::text_preview)
            .collect::<String>();
        let images = content
            .iter()
            .filter_map(|part| match part {
                Content::Image(media) | Content::Sticker(media) => Some(media.clone()),
                _ => None,
            })
            .take(3)
            .collect::<Vec<_>>();
        crate::chat_ui::quote(cx)
            .children(images.into_iter().map(|media| {
                let path = self.media.get(&media.id).cloned();
                if path.is_none()
                    && !self.failed_downloads.contains(&media.id)
                    && self.downloads.insert(media.id.clone())
                {
                    self.command(Command::Download(media.clone()));
                }
                div().size_12().flex_shrink_0().bg(cx.theme().muted).child(
                    crate::image_loading::LoadingImage::new(
                        SharedString::from(format!("quote:{}", media.id)),
                        path,
                    )
                    .square()
                    .failed(self.failed_downloads.contains(&media.id))
                    .size_full(),
                )
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
                            .child(sender),
                    )
                    .child(div().truncate().child(if text.is_empty() {
                        t!("common.message").into_owned()
                    } else {
                        text
                    })),
            )
            .into_any_element()
    }
    fn row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(message) = self
            .rows
            .get(index)
            .and_then(|id| self.timeline.messages.get(id))
            .cloned()
        else {
            return div().into_any_element();
        };
        let continued = index
            .checked_sub(1)
            .and_then(|index| self.rows.get(index))
            .and_then(|id| self.timeline.messages.get(id))
            .is_some_and(|previous| chat_adapter::continues_sender(previous, &message));
        let notice = message.content.iter().any(
            |part| matches!(part,Content::Extension(extension) if extension.layout==Layout::Notice),
        );
        let text = message
            .content
            .iter()
            .map(Content::text_preview)
            .collect::<Vec<_>>()
            .join("\n");
        let actions = self
            .descriptor
            .as_ref()
            .map(|d| {
                d.actions
                    .iter()
                    .filter(|action| action.scope == Scope::Message)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let weak = cx.entity().downgrade();
        let target = message.id.clone();
        let reply = message.id.clone();
        let avatar = if continued {
            div().into_any_element()
        } else {
            self.avatar_view(
                message.sender.avatar.as_deref(),
                &message.sender.name,
                36.,
                cx,
            )
        };
        let mut contents = if message.deleted {
            vec![
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("common.recalled"))
                    .into_any_element(),
            ]
        } else {
            message
                .content
                .iter()
                .map(|part| match part {
                    Content::Text(text) => div()
                        .text_sm()
                        .child(crate::message_text::MessageText::new(
                            "adapter-message-text",
                            text.clone(),
                        ))
                        .into_any_element(),
                    Content::Image(media) | Content::Sticker(media) => self.media_view(media, cx),
                    Content::Extension(extension) => self.extension_view(extension, cx),
                    other => div()
                        .text_sm()
                        .child(other.text_preview())
                        .into_any_element(),
                })
                .collect()
        };
        if !message.deleted
            && let Some(reply) = &message.reply
        {
            contents.insert(0, self.reply_preview(reply, cx));
        }
        let progress = self.arrivals.get(&message.id).map_or(1., |started| {
            (cx.background_executor()
                .now()
                .saturating_duration_since(*started)
                .as_secs_f32()
                / 0.24)
                .min(1.)
        });
        if progress < 1. {
            window.request_animation_frame();
        }
        let copy_text = text.clone();
        let images_only = !message.content.is_empty()
            && message
                .content
                .iter()
                .all(|part| matches!(part, Content::Image(_) | Content::Sticker(_)));
        let bubble = crate::chat_ui::bubble(
            message.outgoing,
            !notice && (message.deleted || !images_only),
            cx,
        )
        .id(SharedString::from(format!("bubble:{}", message.id.id)))
        .debug_selector(|| "adapter-message-bubble".into())
        .min_w_0()
        .when(notice, |bubble| {
            bubble.text_color(cx.theme().muted_foreground).text_xs()
        })
        .children(contents)
        .context_menu(move |menu, _, _| {
            let owner = weak.clone();
            let copy = text.clone();
            let menu = menu.item(PopupMenuItem::new(t!("common.copy")).on_click(
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())),
            ));
            let reply = reply.clone();
            let menu = menu.item(PopupMenuItem::new(t!("common.reply")).on_click(
                move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.reply = Some(reply.clone());
                        cx.notify();
                    });
                },
            ));
            actions.iter().fold(menu, |menu, action| {
                let owner = weak.clone();
                let action = action.clone();
                let target = target.clone();
                menu.item(PopupMenuItem::new(action.label.clone()).on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.open_action(
                                action.clone(),
                                ExtensionRequest {
                                    action: action.id.clone(),
                                    chat: Some(target.chat.clone()),
                                    message: Some(target.clone()),
                                    user: None,
                                    data: Default::default(),
                                },
                                window,
                                cx,
                            )
                        });
                    },
                ))
            })
        });
        let fade = self.fades.get(&message.id);
        let (from, to, generation) = fade
            .map(|fade| (fade.from, fade.to, fade.generation))
            .unwrap_or((0., 0., 0));
        let hover = message.id.clone();
        let reply_to = message.id.clone();
        let time = chrono::DateTime::from_timestamp_millis(message.time_ms)
            .map(|time| {
                time.with_timezone(&chrono::Local)
                    .format("%H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        let time_label = div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(time)
            .with_animation(
                SharedString::from(format!("adapter-time:{}:{generation}", message.id.id)),
                Animation::new(Duration::from_millis(80)),
                move |label, progress| label.opacity(from + (to - from) * progress),
            );
        let actions = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new(SharedString::from(format!("reply:{}", message.id.id)))
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Reply)
                    .tooltip(t!("common.reply"))
                    .accessibility_label(t!("common.reply"))
                    .disabled(message.deleted)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reply = Some(reply_to.clone());
                        this.composer
                            .update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    })),
            )
            .child(
                Button::new(SharedString::from(format!("copy:{}", message.id.id)))
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Copy)
                    .tooltip(t!("common.copy"))
                    .accessibility_label(t!("common.copy"))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()))
                    }),
            )
            .when(continued, |actions| actions.child(time_label))
            .with_animation(
                SharedString::from(format!("adapter-actions:{}:{generation}", message.id.id)),
                Animation::new(Duration::from_millis(80)),
                move |row, progress| {
                    let opacity = from + (to - from) * progress;
                    row.opacity(opacity)
                        .when(opacity <= 0., |row| row.invisible())
                },
            );
        let column = crate::chat_ui::sender_column(message.outgoing, self.phone)
            .when(!continued, |column| {
                column.child(
                    div()
                        .debug_selector({
                            let id = message.id.id.clone();
                            move || format!("adapter-sender-header:{id}")
                        })
                        .flex()
                        .gap_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .when(message.outgoing, |header| header.flex_row_reverse())
                        .child(message.sender.name.clone())
                        .child(
                            div()
                                .child(
                                    chrono::DateTime::from_timestamp_millis(message.time_ms)
                                        .map(|time| {
                                            time.with_timezone(&chrono::Local)
                                                .format("%H:%M")
                                                .to_string()
                                        })
                                        .unwrap_or_default(),
                                )
                                .with_animation(
                                    SharedString::from(format!(
                                        "adapter-header-time:{}:{generation}",
                                        message.id.id
                                    )),
                                    Animation::new(Duration::from_millis(80)),
                                    move |label, progress| {
                                        label.opacity(from + (to - from) * progress)
                                    },
                                ),
                        ),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("proximity:{}", message.id.id)))
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .p_2()
                    .m(px(-8.))
                    .when(message.outgoing, |col| col.items_end())
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_message_hover(&hover, *hovered, cx)
                    }))
                    .child(bubble)
                    .child(actions),
            );
        let lane = crate::chat_ui::message_lane(message.outgoing)
            .when(!continued, |row| row.child(avatar))
            .when(continued, |row| row.child(div().w_9().flex_shrink_0()))
            .child(column);
        div()
            .id(SharedString::from(format!("row:{}", message.id.id)))
            .w_full()
            .when(!self.phone, |row| row.px_5())
            .when(self.phone, |row| row.px_3())
            .when(!continued, |row| row.py_2())
            .when(continued, |row| row.pt_0().pb_1())
            .opacity(crate::ui_motion::page_progress(progress))
            .when(!notice, |row| row.child(lane))
            .when(notice, |row| {
                row.flex()
                    .justify_center()
                    .child(self.extension_notice(&message, cx))
            })
            .into_any_element()
    }
    fn extension_notice(&mut self, message: &Message, cx: &mut Context<Self>) -> AnyElement {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .children(message.content.iter().map(|part| match part {
                Content::Extension(extension) => self.extension_view(extension, cx),
                part => div().child(part.text_preview()).into_any_element(),
            }))
            .into_any_element()
    }
    fn auth_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(320.))
                    .max_w(relative(0.9))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                            self.descriptor
                                .as_ref()
                                .map_or("", |d| d.name.as_str())
                                .to_owned(),
                        ),
                    )
                    .when(self.protocol_switch.is_some(), |panel| {
                        panel.child(
                            Button::new("adapter-platform")
                                .debug_selector(|| "adapter-platform".into())
                                .ghost()
                                .icon(gpui_kit::assets::IconName::ChevronLeft)
                                .accessibility_label(t!("chat.back"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(switch) = this.protocol_switch.clone() {
                                        switch(window, cx);
                                    }
                                })),
                        )
                    })
                    .when_some(self.form.as_ref(), |panel, form| {
                        panel.children(form.fields.iter().map(|(field, input)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().child(field_label(field)))
                                .child(
                                    Input::new(input)
                                        .aria_label(field_label(field))
                                        .disabled(self.login.busy),
                                )
                        }))
                    })
                    .when_some(
                        match &self.login.state {
                            AuthState::Choice { step, choices } => {
                                Some((step.clone(), choices.clone()))
                            }
                            _ => None,
                        },
                        |panel, (step, choices)| {
                            panel.children(choices.into_iter().map(|choice| {
                                let step = step.clone();
                                Button::new(SharedString::from(format!(
                                    "auth-choice:{}",
                                    choice.id
                                )))
                                .label(choice_label(&choice))
                                .disabled(self.login.busy)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.login.busy = true;
                                        this.error = None;
                                        this.command(Command::Authenticate(AuthInput {
                                            step: step.clone(),
                                            fields: BTreeMap::from([(
                                                "choice".into(),
                                                choice.id.clone(),
                                            )]),
                                        }));
                                        cx.notify();
                                    },
                                ))
                            }))
                        },
                    )
                    .when(matches!(self.login.state, AuthState::Qr { .. }), |panel| {
                        panel.child(
                            div().w_full().flex().justify_center().child(
                                div()
                                    .debug_selector(|| "adapter-login-qr".into())
                                    .size(px(224.))
                                    .bg(rgb(0xffffff))
                                    .rounded_lg()
                                    .when_some(self.qr.as_ref(), |view, (_, image)| {
                                        view.child(img(image.clone()).size_full())
                                    }),
                            ),
                        )
                    })
                    .when_some(
                        match &self.login.state {
                            AuthState::Qr { choices, .. } => Some(choices.clone()),
                            _ => None,
                        },
                        |panel, choices| {
                            panel.children(choices.into_iter().map(|choice| {
                                Button::new(SharedString::from(format!("qr-choice:{}", choice.id)))
                                    .debug_selector({
                                        let id = choice.id.clone();
                                        move || format!("qr-choice:{id}")
                                    })
                                    .ghost()
                                    .label(choice_label(&choice))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.error = None;
                                        this.login.busy = true;
                                        this.command(Command::Authenticate(AuthInput {
                                            step: choice.id.clone(),
                                            fields: Default::default(),
                                        }));
                                        cx.notify();
                                    }))
                            }))
                        },
                    )
                    .when_some(
                        match &self.login.state {
                            AuthState::Browser { url } => Some(url.clone()),
                            _ => None,
                        },
                        |panel, url| {
                            panel.child(
                                Button::new("open-login-browser")
                                    .label(t!("login.open_verification"))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        },
                    )
                    .when(
                        self.form.is_none()
                            && matches!(self.login.state, AuthState::Disconnected)
                            && self.error.is_none(),
                        |panel| panel.child(Spinner::new()),
                    )
                    .when(self.form.is_none() && self.error.is_some(), |panel| {
                        panel.child(
                            Button::new("adapter-auth-retry")
                                .label(t!("common.retry"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    this.login.busy = true;
                                    this.command(Command::Authenticate(AuthInput {
                                        step: "resume".into(),
                                        fields: Default::default(),
                                    }));
                                    cx.notify();
                                })),
                        )
                    })
                    .when(self.form.is_some(), |panel| {
                        panel.child(
                            Button::new("adapter-auth-submit")
                                .debug_selector(|| "adapter-auth-submit".into())
                                .primary()
                                .label(t!("adapter.continue"))
                                .disabled(self.login.busy || self.form.is_none())
                                .loading(self.login.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.submit_auth(cx))),
                        )
                    }),
            )
            .into_any_element()
    }
    fn account_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let actions = self
            .descriptor
            .as_ref()
            .map(|descriptor| {
                descriptor
                    .actions
                    .iter()
                    .filter(|action| action.scope == Scope::Account)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let weak = cx.entity().downgrade();
        div()
            .id("adapter-account-control")
            .flex()
            .items_center()
            .when(!actions.is_empty(), |row| {
                row.child(
                    Button::new("account-options")
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .dropdown_menu(move |menu, _, _| {
                            actions.iter().fold(menu, |menu, action| {
                                let owner = weak.clone();
                                let action = action.clone();
                                menu.item(PopupMenuItem::new(action.label.clone()).on_click(
                                    move |_, window, cx| {
                                        let _ = owner.update(cx, |this, cx| {
                                            this.open_action(
                                                action.clone(),
                                                ExtensionRequest {
                                                    action: action.id.clone(),
                                                    chat: None,
                                                    message: None,
                                                    user: None,
                                                    data: Default::default(),
                                                },
                                                window,
                                                cx,
                                            )
                                        });
                                    },
                                ))
                            })
                        }),
                )
            })
            .into_any_element()
    }
    fn chat_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.search.read(cx).value().to_lowercase();
        let mut chats = self
            .chats
            .iter()
            .filter(|chat| self.search_index.borrow_mut().matches(&chat.title, &query))
            .cloned()
            .collect::<Vec<_>>();
        chats.sort_by_key(|chat| std::cmp::Reverse(chat.last_activity_ms));
        let owner = cx.entity();
        let rows = uniform_list("adapter-chats", chats.len(), move |range, _, cx| {
            owner.update(cx, |this, cx| {
                range
                    .map(|index| {
                        let chat = &chats[index];
                        let id = chat.id.clone();
                        let preview = this
                            .timeline
                            .in_chat(&chat.id)
                            .last()
                            .map(|message| {
                                let text = message
                                    .content
                                    .iter()
                                    .map(Content::text_preview)
                                    .collect::<String>();
                                if chat.kind == chat_adapter::ChatKind::Group && !message.outgoing {
                                    format!("{}: {text}", message.sender.name)
                                } else {
                                    text
                                }
                            })
                            .unwrap_or_default();
                        let avatar = this.avatar_view(chat.avatar.as_deref(), &chat.title, 36., cx);
                        crate::chat_ui::conversation_row(
                            SharedString::from(format!("chat:{}", chat.id.id)),
                            this.selected.as_ref() == Some(&chat.id),
                            cx,
                        )
                        .debug_selector(|| "adapter-chat-row".into())
                        .accessibility_label(t!("chat.open", name = chat.title))
                        .child(crate::chat_ui::conversation_row_content(
                            avatar,
                            chat.title.clone(),
                            preview,
                            chat.unread as usize,
                            cx,
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select(id.clone(), window, cx)
                        }))
                        .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
        });
        crate::chat_ui::conversation_list(self.phone, cx)
            .when(self.phone, |view| {
                view.child(
                    div()
                        .px_4()
                        .py_3()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(t!("chat.title"))
                        .child(self.account_control(cx)),
                )
            })
            .child(
                div()
                    .p_3()
                    .child(Input::new(&self.search).aria_label(t!("chat.search_conversations"))),
            )
            .child(rows.flex_1().min_h_0().px_1())
            .when(self.chat_cursor.is_some(), |view| {
                view.child(
                    Button::new("more-chats")
                        .ghost()
                        .label(t!("adapter.more"))
                        .on_click(cx.listener(|this, _, _, _| {
                            if let Some(cursor) = this.chat_cursor.take() {
                                this.command(Command::Conversations(PageRequest {
                                    cursor: Some(cursor),
                                    limit: 50,
                                }));
                            }
                        })),
                )
            })
            .into_any_element()
    }
    fn navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_kit::assets::IconName;
        crate::chat_ui::navigation(self.phone, cx)
            .children(
                [
                    (
                        Page::Chat,
                        "nav-chat",
                        t!("chat.title").into_owned(),
                        IconName::MessageCircle,
                    ),
                    (
                        Page::Settings,
                        "nav-settings",
                        t!("settings.title").into_owned(),
                        IconName::Settings,
                    ),
                ]
                .into_iter()
                .map(|(page, id, label, icon)| {
                    div().debug_selector(move || id.into()).child(
                        crate::chat_ui::navigation_button(
                            id,
                            label.into(),
                            icon,
                            self.page == page,
                            self.phone,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.page = page;
                                if this.phone {
                                    this.phone_conversation = false;
                                }
                                this.transition = None;
                                window.blur(cx);
                                cx.notify();
                            },
                        )),
                    )
                }),
            )
            .when(self.protocol_switch.is_some(), |nav| {
                nav.child(
                    Button::new("nav-accounts")
                        .debug_selector(|| "nav-accounts".into())
                        .ghost()
                        .icon(IconName::UserRound)
                        .tooltip(t!("accounts.title"))
                        .accessibility_label(t!("accounts.title"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(switch) = this.protocol_switch.clone() {
                                switch(window, cx);
                            }
                        })),
                )
            })
            .into_any_element()
    }
    fn main_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let page = if self.page == Page::Settings {
            crate::settings_ui::page(self, cx)
        } else if self.phone {
            if self.phone_conversation {
                self.conversation_panel(cx)
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
                .child(self.conversation_panel(cx))
                .into_any_element()
        };
        let body = div().flex().flex_1().min_w_0().min_h_0();
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
    fn conversation_view(&self, phone: bool, cx: &mut Context<Self>) -> AnyElement {
        let Some(selected) = &self.selected else {
            return div().flex_1().into_any_element();
        };
        let title = self
            .chats
            .iter()
            .find(|chat| &chat.id == selected)
            .map_or("", |chat| chat.title.as_str())
            .to_owned();
        let owner = cx.entity();
        let timeline = list(self.list.clone(), move |index, window, cx| {
            owner.update(cx, |this, cx| this.row(index, window, cx))
        })
        .flex_1()
        .min_h_0();
        let actions = self
            .descriptor
            .as_ref()
            .map(|d| {
                d.actions
                    .iter()
                    .filter(|a| a.scope == Scope::Conversation)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                crate::chat_ui::conversation_header(phone, cx)
                    .when(phone, |header| {
                        header.child(
                            Button::new("adapter-back")
                                .icon(gpui_kit::assets::IconName::ChevronLeft)
                                .debug_selector(|| "adapter-back".into())
                                .ghost()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.phone_conversation = false;
                                    this.transition = None;
                                    window.blur(cx);
                                    window.dismiss_virtual_keyboard();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    ),
            )
            .when(self.loading.contains(selected), |view| {
                view.child(
                    div()
                        .h(px(28.))
                        .flex()
                        .justify_center()
                        .child(Spinner::new()),
                )
            })
            .child(timeline)
            .when_some(self.reply.as_ref(), |view, reply| {
                view.child(
                    div()
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().text_xs().truncate().child(
                                self.timeline
                                    .messages
                                    .get(reply)
                                    .map(|m| {
                                        format!(
                                            "{}: {}",
                                            m.sender.name,
                                            m.content
                                                .iter()
                                                .map(Content::text_preview)
                                                .collect::<String>()
                                        )
                                    })
                                    .unwrap_or_default(),
                            ),
                        )
                        .child(
                            Button::new("cancel-reply")
                                .icon(gpui_kit::assets::IconName::X)
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.reply = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                crate::chat_ui::composer(phone, cx)
                    .child(
                        div().flex().items_center().gap_1().child(
                            Button::new("adapter-emoji")
                                .ghost()
                                .small()
                                .icon(gpui_kit::assets::IconName::FaceSlightlySmiling)
                                .tooltip(t!("expressions.title"))
                                .accessibility_label(t!("expressions.title"))
                                .dropdown_menu({
                                    let owner = cx.entity();
                                    move |menu, _, _| {
                                        ["🙂", "😀", "❤️", "👍", "🎉", "🤔"].into_iter().fold(
                                            menu,
                                            |menu, emoji| {
                                                let owner = owner.clone();
                                                menu.item(PopupMenuItem::new(emoji).on_click(
                                                    move |_, window, cx| {
                                                        owner.update(cx, |this, cx| {
                                                            this.composer.update(
                                                                cx,
                                                                |input, cx| {
                                                                    input.focus(window, cx);
                                                                    input
                                                                        .replace(emoji, window, cx);
                                                                },
                                                            );
                                                            cx.notify();
                                                        })
                                                    },
                                                ))
                                            },
                                        )
                                    }
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .gap_2()
                            .when(phone, |row| row.items_end())
                            .when(!phone, |row| row.flex_col())
                            .child(
                                div()
                                    .debug_selector(|| "composer-editor".into())
                                    .min_w_0()
                                    .when(phone, |editor| editor.flex_1())
                                    .child(
                                        Textarea::new(&self.composer)
                                            .appearance(false)
                                            .when(!phone, |input| input.h_24())
                                            .aria_label(t!("chat.input")),
                                    ),
                            )
                            .child(
                                div().flex().justify_end().child(
                                    Button::new("adapter-send")
                                        .debug_selector(|| "adapter-send".into())
                                        .primary()
                                        .when(phone, |button| {
                                            button
                                                .size(px(38.))
                                                .icon(gpui_kit::assets::IconName::ArrowUp)
                                        })
                                        .when(!phone, |button| button.label(t!("common.send")))
                                        .disabled(self.composer.read(cx).value().trim().is_empty())
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.send(window, cx)
                                            }),
                                        ),
                                ),
                            ),
                    )
                    .children(actions.into_iter().map(|action| {
                        Button::new(SharedString::from(action.id.clone()))
                            .ghost()
                            .label(action.label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_action(
                                    action.clone(),
                                    ExtensionRequest {
                                        action: action.id.clone(),
                                        chat: this.selected.clone(),
                                        message: None,
                                        user: None,
                                        data: Default::default(),
                                    },
                                    window,
                                    cx,
                                )
                            }))
                    })),
            )
            .into_any_element()
    }
    fn action_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.action else {
            return div().into_any_element();
        };
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(0x00000055))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(360.))
                    .max_w(relative(0.9))
                    .p_4()
                    .rounded_lg()
                    .bg(cx.theme().background)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(form.action.label.clone())
                    .children(
                        form.fields
                            .iter()
                            .map(|(field, input)| Input::new(input).aria_label(field_label(field))),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("action-cancel")
                                    .ghost()
                                    .label(t!("common.cancel"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.action = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("action-submit")
                                    .primary()
                                    .label(t!("common.send"))
                                    .on_click(cx.listener(|this, _, _, cx| this.submit_action(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn sync_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.main_window {
            return;
        }
        let kind = if matches!(self.login.state, AuthState::Ready { .. }) {
            crate::window_chrome::AppWindowKind::Main
        } else {
            crate::window_chrome::AppWindowKind::Login
        };
        crate::window_chrome::resize_to(kind, window, cx);
    }
}

impl Render for AdapterView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = rust_i18n::locale().to_string();
        if self.applied_locale != locale {
            self.applied_locale = locale;
            self.composer.update(cx, |input, cx| {
                input.set_placeholder(t!("chat.input"), window, cx)
            });
            self.search.update(cx, |input, cx| {
                input.set_placeholder(t!("chat.search_conversations"), window, cx)
            });
        }
        self.sync_window(window, cx);
        let phone = self.mobile_platform && crate::chat_ui::phone_layout(window.viewport_size());
        if self.phone != phone {
            self.phone = phone;
            self.transition = None;
            self.composer
                .update(cx, |input, cx| input.set_submit_on_enter(!phone, cx));
        }
        self.advance_transition(window, cx);
        let ready = matches!(self.login.state, AuthState::Ready { .. });
        let profile = self.profile.clone();
        let profile_avatar = profile
            .as_ref()
            .map(|user| self.avatar_view(user.avatar.as_deref(), &user.name, 36., cx));
        let content = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .when(!self.mobile_platform, |view| {
                view.child(
                    crate::window_chrome::title_bar(cx)
                        .child(
                            crate::chat_ui::identity(window)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(switch) = this.protocol_switch.clone() {
                                        switch(window, cx);
                                    }
                                }))
                                .when_some(profile_avatar, |header, avatar| header.child(avatar))
                                .child(
                                    self.profile
                                        .as_ref()
                                        .map_or("", |user| user.name.as_str())
                                        .to_owned(),
                                ),
                        )
                        .child(div().flex_1().min_w_0())
                        .child(self.account_control(cx)),
                )
            })
            .when(!ready, |view| view.child(self.auth_view(cx)))
            .when(ready, |view| view.child(self.main_page(cx)))
            .when_some(self.error.as_ref(), |view, error| {
                view.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
            })
            .when(self.action.is_some(), |view| {
                view.child(self.action_view(cx))
            });
        crate::window_chrome::surface(content, window, cx)
    }
}

impl crate::settings_ui::Host for AdapterView {
    fn preferences(&self) -> Preferences {
        self.preferences
    }
    fn section(&self) -> SettingsSection {
        self.settings_section
    }
    fn phone(&self) -> bool {
        self.phone
    }
    fn set_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.settings_section = section;
        cx.notify();
    }
    fn change_preferences(
        &mut self,
        preferences: Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferences = preferences;
        crate::settings::apply(preferences, cx);
        if preferences.appearance == crate::settings::Appearance::System {
            crate::settings::apply_theme(preferences.appearance, window.appearance(), cx);
        }
        self.command(Command::SavePreferences(preferences));
        cx.notify();
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::{AdapterView, Backend, Command, Update};
    use chat_adapter::{
        AccountId, AuthState, Content, Conversation, DisplayPart, Event, ExtensionContent, Field,
        FieldKind, Layout, Message, MessageId, SendOutcome, User,
    };
    use gpui::{
        AnyWindowHandle, AppContext, Bounds, Entity, TestAppContext, VisualTestContext,
        WindowBounds, WindowOptions, point, px, size,
    };
    use std::time::Duration;
    use tokio::sync::{mpsc, watch};
    type Fixture = (
        AnyWindowHandle,
        Entity<AdapterView>,
        mpsc::UnboundedReceiver<Command>,
    );
    fn fixture(cx: &mut TestAppContext) -> Fixture {
        let (commands, requests) = mpsc::unbounded_channel();
        let (_sender, updates) = mpsc::unbounded_channel();
        let (_finished, stopped) = watch::channel(false);
        let (handle, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::window_chrome::init(cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1100.), px(760.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = AdapterView::new(
                            Backend {
                                commands,
                                updates: Some(updates),
                                stopped,
                            },
                            window,
                            cx,
                        );
                        view.main_window = true;
                        view
                    })
                },
            )
            .unwrap()
        });
        (handle, view, requests)
    }
    fn frames(cx: &mut TestAppContext, handle: AnyWindowHandle) {
        for _ in 0..3 {
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
    #[gpui::test]
    fn unified_shell_uses_qq_geometry_and_shared_settings_on_desktop_and_phone(
        cx: &mut TestAppContext,
    ) {
        let (handle, view, mut requests) = fixture(cx);
        let account = AccountId {
            adapter: "telegram".into(),
            id: "one".into(),
        };
        let chat = account.chat("room");
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply(
                    Update::Auth(AuthState::Ready {
                        account: account.clone(),
                        user: User {
                            id: "me".into(),
                            name: "Me".into(),
                            avatar: None,
                        },
                    }),
                    window,
                    cx,
                );
                view.chats = vec![Conversation {
                    id: chat.clone(),
                    kind: chat_adapter::ChatKind::Direct,
                    title: "Alice".into(),
                    avatar: None,
                    unread: 1,
                    last_activity_ms: 1,
                }];
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, handle);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let navigation = visual.debug_bounds("navigation").unwrap();
        let chats = visual.debug_bounds("chat-list").unwrap();
        assert_eq!(navigation.size.width, px(52.));
        assert_eq!(chats.size.width, px(288.));
        assert_eq!(chats.left(), navigation.right());
        assert_eq!(
            visual.debug_bounds("account-identity").unwrap().size.width,
            px(288.)
        );
        let settings = visual.debug_bounds("nav-settings").unwrap();
        visual.simulate_click(settings.center(), Default::default());
        frames(cx, handle);
        let mut visual = VisualTestContext::from_window(handle, cx);
        assert!(visual.debug_bounds("settings-page").is_some());
        assert!(visual.debug_bounds("language-dropdown").is_some());
        let appearance = visual.debug_bounds("settings-appearance").unwrap();
        visual.simulate_click(appearance.center(), Default::default());
        frames(cx, handle);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let dark = visual.debug_bounds("appearance-dark").unwrap();
        assert!(dark.size.width >= px(160.));
        visual.simulate_click(dark.center(), Default::default());
        assert!(std::iter::from_fn(||requests.try_recv().ok()).any(|request|matches!(request,Command::SavePreferences(preferences) if preferences.appearance==crate::settings::Appearance::Dark)));
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.mobile_platform = true;
                view.page = crate::chat_ui::Page::Chat;
                cx.notify();
            })
        });
        cx.simulate_window_resize(handle, size(px(390.), px(844.)));
        frames(cx, handle);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let navigation = visual.debug_bounds("navigation").unwrap();
        assert_eq!(navigation.size.height, px(56.));
        assert_eq!(navigation.size.width, px(390.));
        assert!(visual.debug_bounds("account-identity").is_none());
        let row = visual.debug_bounds("adapter-chat-row").unwrap();
        visual.simulate_click(row.center(), Default::default());
        frames(cx, handle);
        let mut visual = VisualTestContext::from_window(handle, cx);
        assert!(visual.debug_bounds("navigation").is_none());
        assert!(visual.debug_bounds("chat-list").is_none());
        let back = visual.debug_bounds("adapter-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        frames(cx, handle);
        let mut visual = VisualTestContext::from_window(handle, cx);
        assert!(visual.debug_bounds("navigation").is_some());
        assert!(visual.debug_bounds("chat-list").is_some());
    }
    #[gpui::test]
    fn qr_challenge_refreshes_in_place_and_can_switch_to_phone_login(cx: &mut TestAppContext) {
        let (handle, view, mut requests) = fixture(cx);
        let challenge = |token: &str| AuthState::Qr {
            url: format!("tg://login?token={token}"),
            expires_ms: Some(1000),
            choices: vec![chat_adapter::LoginChoice {
                id: "phone_login".into(),
                label: "Phone".into(),
            }],
        };
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply(Update::Auth(challenge("first")), window, cx);
                let first = view.qr.as_ref().unwrap().1.clone();
                view.apply(Update::Auth(challenge("first")), window, cx);
                assert!(std::sync::Arc::ptr_eq(&first, &view.qr.as_ref().unwrap().1));
                view.apply(Update::Auth(challenge("second")), window, cx);
                assert!(!std::sync::Arc::ptr_eq(
                    &first,
                    &view.qr.as_ref().unwrap().1
                ));
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, handle);
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        assert!(visual.debug_bounds("adapter-login-qr").is_some());
        let button = visual.debug_bounds("qr-choice:phone_login").unwrap();
        visual.simulate_click(button.center(), Default::default());
        let Command::Authenticate(input) = requests.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(input.step, "phone_login");
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply(
                    Update::Auth(AuthState::Form {
                        step: "phone".into(),
                        fields: vec![Field {
                            id: "phone".into(),
                            label: "Phone".into(),
                            kind: FieldKind::Phone,
                            required: true,
                        }],
                    }),
                    window,
                    cx,
                );
                assert!(view.qr.is_none());
                assert!(view.form.is_some());
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, handle);
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        assert!(visual.debug_bounds("adapter-login-qr").is_none());
        assert!(visual.debug_bounds("adapter-auth-submit").is_some());
    }
    #[gpui::test]
    fn consecutive_messages_keep_individual_bubbles_without_repeated_sender_headers(
        cx: &mut TestAppContext,
    ) {
        let (handle, view, _requests) = fixture(cx);
        let account = AccountId {
            adapter: "matrix".into(),
            id: "slot".into(),
        };
        let chat = account.chat("room");
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply(
                    Update::Auth(AuthState::Ready {
                        account: account.clone(),
                        user: User {
                            id: "me".into(),
                            name: "Me".into(),
                            avatar: None,
                        },
                    }),
                    window,
                    cx,
                );
                view.selected = Some(chat.clone());
                for index in 1..=3 {
                    view.timeline.apply(&Event::Message(Message {
                        id: MessageId {
                            chat: chat.clone(),
                            id: index.to_string(),
                        },
                        client_id: None,
                        sender: User {
                            id: if index == 3 { "bob" } else { "alice" }.into(),
                            name: "Peer".into(),
                            avatar: None,
                        },
                        time_ms: index,
                        outgoing: false,
                        content: vec![Content::Text(format!("Message {index}"))],
                        reply: None,
                        deleted: false,
                    }));
                }
                view.refresh_rows();
                cx.notify();
            })
        })
        .unwrap();
        frames(cx, handle);
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        assert!(visual.debug_bounds("adapter-sender-header:1").is_some());
        assert!(visual.debug_bounds("adapter-sender-header:2").is_none());
        assert!(visual.debug_bounds("adapter-sender-header:3").is_some());
        cx.update(|cx| assert_eq!(view.read(cx).timeline.in_chat(&chat).len(), 3));
    }
    #[gpui::test]
    fn unknown_platform_notices_render_and_optimistic_sends_reconcile_in_the_same_view(
        cx: &mut TestAppContext,
    ) {
        let (handle, view, mut requests) = fixture(cx);
        let account = AccountId {
            adapter: "future-platform".into(),
            id: "one".into(),
        };
        let chat = account.chat("room");
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply(
                    Update::Auth(AuthState::Ready {
                        account: account.clone(),
                        user: User {
                            id: "me".into(),
                            name: "Me".into(),
                            avatar: None,
                        },
                    }),
                    window,
                    cx,
                );
                view.apply(
                    Update::Conversations {
                        append: false,
                        page: chat_adapter::Page {
                            items: vec![Conversation {
                                id: chat.clone(),
                                kind: chat_adapter::ChatKind::Direct,
                                title: "Alice".into(),
                                avatar: None,
                                unread: 0,
                                last_activity_ms: 1,
                            }],
                            next: None,
                        },
                    },
                    window,
                    cx,
                );
                view.select(chat.clone(), window, cx);
                let content = Content::Extension(ExtensionContent {
                    kind: "unknown.poke".into(),
                    version: 4,
                    data: serde_json::json!({"unknown":"retained"}),
                    layout: Layout::Notice,
                    display: vec![DisplayPart::Text("Alice tapped you".into())],
                });
                view.apply(
                    Update::Event(Event::Message(Message {
                        id: MessageId {
                            chat: chat.clone(),
                            id: "one".into(),
                        },
                        client_id: None,
                        sender: User {
                            id: "alice".into(),
                            name: "Alice".into(),
                            avatar: None,
                        },
                        time_ms: 1,
                        outgoing: false,
                        content: vec![content],
                        reply: None,
                        deleted: false,
                    })),
                    window,
                    cx,
                );
            })
        })
        .unwrap();
        frames(cx, handle);
        assert!(
            VisualTestContext::from_window(handle, cx)
                .debug_bounds("adapter-extension")
                .is_some()
        );
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.composer
                    .update(cx, |input, cx| input.set_value("hello", window, cx));
                view.send(window, cx);
                assert_eq!(view.timeline.in_chat(&chat).len(), 2);
                assert!(view.composer.read(cx).value().is_empty());
            })
        })
        .unwrap();
        let send = std::iter::from_fn(|| requests.try_recv().ok())
            .find_map(|request| {
                if let Command::Send(request) = request {
                    Some(request)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(send.chat, chat);
        assert_eq!(send.content[0].text_preview(), "hello");
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                let pending = view
                    .timeline
                    .in_chat(&chat)
                    .into_iter()
                    .find(|message| message.outgoing)
                    .unwrap()
                    .clone();
                let mut confirmed = pending.clone();
                confirmed.id.id = "server".into();
                view.apply(
                    Update::Sent {
                        client_id: send.client_id,
                        result: Ok(SendOutcome::confirmed(confirmed.clone())),
                    },
                    window,
                    cx,
                );
                view.apply(Update::Event(Event::Message(confirmed)), window, cx);
                assert_eq!(view.timeline.in_chat(&chat).len(), 2);
                assert!(view.pending.is_empty());
                assert!(!view.timeline.messages.contains_key(&pending.id));
            })
        })
        .unwrap();
        frames(cx, handle);
    }
    #[gpui::test]
    fn adapter_supplied_login_fields_submit_without_client_specific_auth_branches(
        cx: &mut TestAppContext,
    ) {
        let (handle, view, mut requests) = fixture(cx);
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.apply(
                    Update::Auth(AuthState::Form {
                        step: "custom-step".into(),
                        fields: vec![Field {
                            id: "custom-token".into(),
                            label: "Token".into(),
                            kind: FieldKind::Secret,
                            required: true,
                        }],
                    }),
                    window,
                    cx,
                );
                view.submit_auth(cx);
                assert!(!view.login.busy);
                let input = view.form.as_ref().unwrap().fields[0].1.clone();
                input.update(cx, |input, cx| input.set_value("value", window, cx));
                view.submit_auth(cx);
                assert!(view.login.busy);
            })
        })
        .unwrap();
        let Command::Authenticate(input) = requests.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(input.step, "custom-step");
        assert_eq!(input.fields["custom-token"], "value");
        frames(cx, handle);
    }
}
