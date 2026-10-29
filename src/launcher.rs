pub use crate::accounts::Protocol;
use crate::accounts::{Catalog, Profile, SessionHooks};
use chat_adapter::{AuthState, LoginSnapshot};
use gpui_kit::{
    component::{ActiveTheme, button::Button},
    prelude::FluentBuilder,
    *,
};
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc, sync::Arc};
pub type ProtocolSwitch = Rc<dyn Fn(&mut Window, &mut App)>;
struct Entry {
    profile: Profile,
    view: AnyView,
    login: LoginSnapshot,
    close: Rc<dyn Fn()>,
}
pub struct Launcher {
    root: PathBuf,
    catalog: Catalog,
    entries: BTreeMap<String, Entry>,
    selected: Option<String>,
    picker: bool,
    error: Option<String>,
    catalog_valid: bool,
}
impl Launcher {
    pub fn chooser(root: PathBuf) -> Self {
        match Catalog::load(&root) {
            Ok(catalog) => Self {
                root,
                catalog,
                entries: BTreeMap::new(),
                selected: None,
                picker: true,
                error: None,
                catalog_valid: true,
            },
            Err(error) => Self {
                root,
                catalog: Catalog::default(),
                entries: BTreeMap::new(),
                selected: None,
                picker: true,
                error: Some(format!("{error:#}")),
                catalog_valid: false,
            },
        }
    }
    pub fn new(
        root: PathBuf,
        protocol: Option<Protocol>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::chooser(root);
        for profile in this.catalog.profiles.clone() {
            this.open(profile, window, cx);
        }
        let selected = protocol
            .and_then(|protocol| {
                this.entries
                    .values()
                    .find(|entry| entry.profile.protocol == protocol)
                    .map(|entry| entry.profile.id.clone())
            })
            .or_else(|| {
                if protocol.is_none() {
                    this.catalog
                        .selected
                        .clone()
                        .filter(|id| this.entries.contains_key(id))
                        .or_else(|| this.entries.keys().next().cloned())
                } else {
                    None
                }
            });
        if let Some(id) = selected {
            this.selected = Some(id);
            this.picker = false;
        } else if let Some(protocol) = protocol {
            this.add(protocol, window, cx);
        }
        this
    }
    fn save(&mut self) {
        if !self.catalog_valid {
            return;
        }
        if let Err(error) = self.catalog.save(&self.root) {
            self.error = Some(format!("{error:#}"));
        }
    }
    fn changed(&mut self, id: &str, snapshot: LoginSnapshot, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get_mut(id) else {
            return;
        };
        if let AuthState::Ready { user, .. } = &snapshot.state {
            if entry
                .profile
                .user
                .as_ref()
                .is_some_and(|old| old.id != user.id)
            {
                self.error = Some(rust_i18n::t!("accounts.use_new").into_owned());
                return;
            }
            let modified = entry.profile.user.as_ref() != Some(user);
            entry.profile.user = Some(user.clone());
            if entry.profile.protocol != Protocol::Mock && modified {
                self.catalog.remember(entry.profile.clone());
                self.catalog.selected = self.selected.clone();
                self.save();
            }
        }
        if let Some(entry) = self.entries.get_mut(id) {
            entry.login = snapshot;
        }
        cx.notify();
    }
    fn show_accounts(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.selected.clone()
            && self
                .entries
                .get(&id)
                .is_some_and(|entry| entry.profile.user.is_none())
        {
            if let Some(entry) = self.entries.remove(&id) {
                (entry.close)();
            }
            self.selected = None;
        }
        self.picker = true;
        cx.notify();
    }
    fn add(&mut self, protocol: Protocol, window: &mut Window, cx: &mut Context<Self>) {
        let profile = Profile::fresh(protocol);
        let id = profile.id.clone();
        self.open(profile, window, cx);
        self.selected = Some(id);
        self.picker = false;
        cx.notify();
    }
    fn select(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.entries.contains_key(&id) {
            return;
        }
        self.selected = Some(id.clone());
        self.catalog.selected = Some(id);
        self.picker = false;
        self.save();
        cx.notify();
    }
    fn open(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let id = profile.id.clone();
        let owner = cx.entity().downgrade();
        let observer_owner = owner.clone();
        let observer_id = id.clone();
        let hooks = SessionHooks {
            on_login: Rc::new(move |snapshot, cx| {
                let owner = observer_owner.clone();
                let id = observer_id.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |this, cx| this.changed(&id, snapshot, cx));
                });
            }),
            accounts: Rc::new(move |_, cx| {
                let owner = owner.clone();
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |this, cx| this.show_accounts(cx));
                });
            }),
        };
        let directory = profile.directory(&self.root);
        let (view, close): (AnyView, Rc<dyn Fn()>) = match profile.protocol {
            Protocol::Qq => {
                let mut backend = crate::backend::Backend::start_bound(
                    directory,
                    profile.user.as_ref().and_then(|user| user.id.parse().ok()),
                );
                let updates = backend.updates.take().expect("QQ updates");
                let commands = backend.commands.clone();
                let backend = Rc::new(RefCell::new(Some(backend)));
                let shutdown = backend.clone();
                let app: &mut App = cx;
                app.on_app_quit(move |_| {
                    if let Some(backend) = shutdown.borrow_mut().take() {
                        backend.shutdown();
                    }
                    async {}
                })
                .detach();
                let expected = profile.user.as_ref().map(|user| user.id.clone());
                let view = cx
                    .new(|cx| {
                        crate::view::AppView::new(commands, updates, window, cx)
                            .with_session_hooks(hooks, expected)
                    })
                    .into();
                let close = Rc::new(move || {
                    if let Some(backend) = backend.borrow_mut().take() {
                        std::thread::spawn(move || backend.shutdown());
                    }
                });
                (view, close)
            }
            protocol => {
                let local = profile.account_id.clone();
                let sdk_directory = directory.join("matrix-sdk");
                let factory: crate::adapters::Factory = match protocol {
                    Protocol::Telegram => Box::new(move |store| {
                        Arc::new(
                            crate::telegram_config::configure(
                                adapter_telegram::TelegramAdapter::new(local, store),
                            )
                            .with_application(
                                adapter_telegram::Application {
                                    name: "Ferrsphen".into(),
                                    version: env!("CARGO_PKG_VERSION").into(),
                                    system_language: rust_i18n::locale().to_string(),
                                },
                            ),
                        )
                    }),
                    Protocol::Matrix => Box::new(move |store| {
                        Arc::new(
                            adapter_matrix::MatrixAdapter::new(local, store, sdk_directory)
                                .with_device_name("Ferrsphen"),
                        )
                    }),
                    _ => Box::new(move |_| Arc::new(chat_adapter::mock::MockAdapter::new(local))),
                };
                let backend = crate::adapters::Backend::start(directory, factory);
                let commands = backend.commands.clone();
                let stopped = backend.stopped.clone();
                let quit = commands.clone();
                let app: &mut App = cx;
                app.on_app_quit(move |_| {
                    let _ = quit.send(crate::adapters::Command::Close);
                    let mut stopped = stopped.clone();
                    async move {
                        while !*stopped.borrow() {
                            if stopped.changed().await.is_err() {
                                break;
                            }
                        }
                    }
                })
                .detach();
                let view = cx
                    .new(|cx| {
                        crate::adapter_view::AdapterView::new(backend, window, cx)
                            .with_session_hooks(hooks)
                    })
                    .into();
                (
                    view,
                    Rc::new(move || {
                        let _ = commands.send(crate::adapters::Command::Close);
                    }),
                )
            }
        };
        self.entries.insert(
            id,
            Entry {
                profile,
                view,
                close,
                login: LoginSnapshot::default(),
            },
        );
    }
    fn picker_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let profiles: Vec<_> = self
            .entries
            .values()
            .filter(|entry| entry.profile.user.is_some())
            .map(|entry| (entry.profile.clone(), entry.login.clone()))
            .collect();
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .flex()
            .flex_col()
            .when(!crate::platform::is_mobile(), |view| {
                view.child(crate::window_chrome::title_bar(cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .id("account-picker")
                            .debug_selector(|| "account-picker".into())
                            .w(px(440.))
                            .max_w(relative(0.9))
                            .max_h(relative(0.9))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .children(profiles.into_iter().map(|(profile, state)| {
                                let id = profile.id.clone();
                                div()
                                    .id(SharedString::from(format!("account:{id}")))
                                    .debug_selector(|| "saved-account".into())
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .cursor_pointer()
                                    .hover(|row| row.bg(cx.theme().muted))
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .w(px(36.))
                                            .h(px(36.))
                                            .rounded_full()
                                            .bg(cx.theme().muted)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                profile
                                                    .label()
                                                    .chars()
                                                    .next()
                                                    .unwrap_or('·')
                                                    .to_string(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .truncate()
                                                    .child(profile.label().to_owned()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(profile.protocol.name()),
                                            ),
                                    )
                                    .when(state.busy, |row| {
                                        row.child(gpui_kit::component::spinner::Spinner::new())
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select(id.clone(), cx)
                                    }))
                            }))
                            .child(
                                div().flex().gap_2().children(
                                    [Protocol::Qq, Protocol::Telegram, Protocol::Matrix]
                                        .into_iter()
                                        .map(|protocol| {
                                            Button::new(protocol.name())
                                                .debug_selector(move || {
                                                    format!(
                                                        "choose-{}",
                                                        protocol.name().to_lowercase()
                                                    )
                                                })
                                                .flex_1()
                                                .label(protocol.name())
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.add(protocol, window, cx)
                                                    },
                                                ))
                                        }),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.selected.as_ref().and_then(|id| self.entries.get(id));
        let ready =
            active.is_some_and(|entry| matches!(entry.login.state, AuthState::Ready { .. }));
        crate::window_chrome::resize_to(
            if ready {
                crate::window_chrome::AppWindowKind::Main
            } else {
                crate::window_chrome::AppWindowKind::Login
            },
            window,
            cx,
        );
        let content = if !self.picker {
            active.map(|entry| {
                div()
                    .size_full()
                    .child(entry.view.clone())
                    .into_any_element()
            })
        } else {
            None
        };
        let content = content.unwrap_or_else(|| {
            crate::window_chrome::surface(self.picker_view(cx), window, cx).into_any_element()
        });
        div()
            .size_full()
            .relative()
            .child(content)
            .when_some(self.error.as_ref(), |view, error| {
                view.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .p_3()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                )
            })
    }
}

#[cfg(all(test, feature = "ui-tests"))]
mod tests {
    use super::{AuthState, Entry, Launcher, LoginSnapshot, Profile, Protocol};
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_kit::{AnyView, AppContext, Context, IntoElement, Render, Window, div};
    use std::cell::Cell;
    use std::rc::Rc;
    struct Probe;
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    #[gpui::test]
    fn empty_catalog_starts_with_platform_choices_in_one_window(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let (window, view) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(
                crate::window_chrome::app_options(crate::window_chrome::AppWindowKind::Login, cx),
                cx,
                |window, cx| cx.new(|cx| Launcher::new(directory.path().into(), None, window, cx)),
            )
            .unwrap()
        });
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let mut visual = VisualTestContext::from_window(window, cx);
        assert!(visual.debug_bounds("account-picker").is_some());
        for selector in ["choose-qq", "choose-telegram", "choose-matrix"] {
            assert!(visual.debug_bounds(selector).is_some(), "{selector}");
        }
        cx.update(|cx| {
            assert!(view.read(cx).entries.is_empty());
            assert_eq!(cx.windows().len(), 1);
        });
    }
    #[gpui::test]
    fn selecting_accounts_keeps_their_views_and_only_cancels_unfinished_login(
        cx: &mut TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let first = Profile::fresh(Protocol::Matrix);
        let second = Profile::fresh(Protocol::Telegram);
        let pending = Profile::fresh(Protocol::Matrix);
        let closed = Rc::new(Cell::new(0));
        let view = cx.new(|_| Launcher::chooser(directory.path().into()));
        cx.update(|cx| {
            for (profile, ready) in [(&first, true), (&second, true), (&pending, false)] {
                let mut profile = profile.clone();
                let state = if ready {
                    let user = chat_adapter::User {
                        id: profile.id.clone(),
                        name: profile.protocol.name().into(),
                        avatar: None,
                    };
                    profile.user = Some(user.clone());
                    AuthState::Ready {
                        account: chat_adapter::AccountId {
                            adapter: profile.protocol.name().to_lowercase(),
                            id: profile.account_id.clone(),
                        },
                        user,
                    }
                } else {
                    AuthState::Disconnected
                };
                let close = closed.clone();
                let child: AnyView = cx.new(|_| Probe).into();
                view.update(cx, |this, _| {
                    this.entries.insert(
                        profile.id.clone(),
                        Entry {
                            profile,
                            view: child,
                            login: LoginSnapshot {
                                state,
                                ..Default::default()
                            },
                            close: Rc::new(move || close.set(close.get() + 1)),
                        },
                    );
                });
            }
            view.update(cx, |this, cx| {
                this.select(first.id.clone(), cx);
                this.show_accounts(cx);
                this.select(second.id.clone(), cx);
                assert_eq!(this.entries.len(), 3);
                assert_eq!(closed.get(), 0);
                this.selected = Some(pending.id.clone());
                this.show_accounts(cx);
                assert_eq!(this.entries.len(), 2);
                assert_eq!(closed.get(), 1);
                this.select(first.id.clone(), cx);
                assert_eq!(this.selected.as_ref(), Some(&first.id));
                assert!(matches!(
                    this.entries[&second.id].login.state,
                    AuthState::Ready { .. }
                ));
            });
        });
    }
}
