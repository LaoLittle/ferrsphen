use crate::settings::{Appearance, Language, Preferences};
use gpui_kit::{
    component::{
        ActiveTheme,
        button::{Button, ButtonVariants},
        menu::{DropdownMenu, PopupMenuItem},
    },
    prelude::FluentBuilder,
    *,
};
use rust_i18n::t;
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    General,
    Appearance,
}
use Section as SettingsSection;
pub trait Host: Sized + 'static {
    fn preferences(&self) -> Preferences;
    fn section(&self) -> Section;
    fn phone(&self) -> bool;
    fn set_section(&mut self, section: Section, cx: &mut Context<Self>);
    fn change_preferences(
        &mut self,
        preferences: Preferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    );
}
pub fn page<T: Host>(host: &T, cx: &mut Context<T>) -> AnyElement {
    let categories = div()
        .debug_selector(|| "settings-categories".into())
        .w(px(if host.phone() { 112. } else { 196. }))
        .h_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .border_r_1()
        .border_color(cx.theme().border)
        .children(
            [
                (
                    SettingsSection::General,
                    "settings-general",
                    "settings.general",
                ),
                (
                    SettingsSection::Appearance,
                    "settings-appearance",
                    "settings.appearance",
                ),
            ]
            .into_iter()
            .map(|(section, id, label)| {
                div().debug_selector(move || id.into()).child(
                    Button::new(id)
                        .ghost()
                        .w_full()
                        .justify_start()
                        .label(t!(label))
                        .when(host.section() == section, |button| {
                            button.bg(cx.theme().accent)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_section(section, cx);
                            cx.notify();
                        })),
                )
            }),
        );
    let heading = match host.section() {
        SettingsSection::General => "settings.general",
        SettingsSection::Appearance => "settings.appearance",
    };
    let details = div()
        .id("settings-details")
        .debug_selector(|| "settings-details".into())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .h_full()
        .overflow_y_scroll()
        .p(px(if host.phone() { 16. } else { 32. }))
        .child(
            div()
                .w_full()
                .max_w(px(720.))
                .flex()
                .flex_col()
                .gap_6()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!(heading)),
                )
                .child(match host.section() {
                    SettingsSection::General => language(host, cx),
                    SettingsSection::Appearance => appearance(host, cx),
                }),
        );
    div()
        .debug_selector(|| "settings-page".into())
        .flex()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .h_full()
        .child(categories)
        .child(details)
        .into_any_element()
}

fn language<T: Host>(host: &T, cx: &mut Context<T>) -> AnyElement {
    let owner = cx.entity();
    let selected = host.preferences().language;
    let label = if selected == Language::System {
        t!("settings.system").into_owned()
    } else {
        selected.native_name().to_owned()
    };
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(t!("settings.language")),
        )
        .child(
            div()
                .debug_selector(|| "language-dropdown".into())
                .w_full()
                .max_w(px(400.))
                .child(
                    Button::new("language-select")
                        .w_full()
                        .justify_between()
                        .label(label)
                        .icon(gpui_kit::assets::IconName::ChevronDown)
                        .accessibility_label(t!("settings.language"))
                        .dropdown_menu(move |mut menu, _, _| {
                            for language in Language::ALL {
                                let label = if language == Language::System {
                                    t!("settings.system").into_owned()
                                } else {
                                    language.native_name().to_owned()
                                };
                                let owner = owner.clone();
                                menu = menu.item(
                                    PopupMenuItem::element(move |_, _| {
                                        div()
                                            .debug_selector(move || language.id().into())
                                            .child(label.clone())
                                    })
                                    .checked(language == selected)
                                    .on_click(
                                        move |_, window, cx| {
                                            owner.update(cx, |this, cx| {
                                                this.change_preferences(
                                                    Preferences {
                                                        language,
                                                        ..this.preferences()
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            });
                                        },
                                    ),
                                );
                            }
                            menu
                        }),
                ),
        )
        .into_any_element()
}

fn appearance_thumbnail(dark: bool) -> Div {
    let background = rgb(if dark { 0x202020 } else { 0xf7f7f7 });
    let muted = rgb(if dark { 0x383838 } else { 0xe3e3e3 });
    let accent = rgb(if dark { 0x426b9a } else { 0x94bdf0 });
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .bg(background)
        .child(
            div()
                .w(px(30.))
                .h_full()
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .bg(muted)
                .child(div().w_full().h_2().rounded_sm().bg(accent))
                .child(div().w_full().h_2().rounded_sm().bg(background)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().w(px(26.)).h_3().rounded_sm().bg(muted))
                .child(div().w(px(30.)).h_3().self_end().rounded_sm().bg(accent)),
        )
}

fn appearance<T: Host>(host: &T, cx: &mut Context<T>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(t!("settings.theme")),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(Appearance::ALL.into_iter().map(|appearance| {
                    let selected = host.preferences().appearance == appearance;
                    let preview = div()
                        .w_full()
                        .h(px(96.))
                        .flex()
                        .overflow_hidden()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().border)
                        .when(appearance != Appearance::Dark, |preview| {
                            preview.child(appearance_thumbnail(false))
                        })
                        .when(appearance != Appearance::Light, |preview| {
                            preview.child(appearance_thumbnail(true))
                        });
                    div().debug_selector(move || appearance.id().into()).child(
                        Button::new(appearance.id())
                            .ghost()
                            .w(px(168.))
                            .h_auto()
                            .p_2()
                            .border_2()
                            .rounded_lg()
                            .border_color(if selected {
                                cx.theme().primary
                            } else {
                                cx.theme().border
                            })
                            .accessibility_label(t!(appearance.label_key()))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_3()
                                    .items_center()
                                    .child(preview)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(if appearance == Appearance::Dark {
                                                gpui_kit::assets::IconName::Moon
                                            } else if appearance == Appearance::Light {
                                                gpui_kit::assets::IconName::Sun
                                            } else {
                                                gpui_kit::assets::IconName::Palette
                                            })
                                            .child(t!(appearance.label_key())),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.change_preferences(
                                    Preferences {
                                        appearance,
                                        ..this.preferences()
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    )
                })),
        )
        .into_any_element()
}
