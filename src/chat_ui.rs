use gpui_kit::{
    component::{
        ActiveTheme, Selectable,
        button::{Button, ButtonVariants},
    },
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Chat,
    Settings,
}

pub fn phone_layout(viewport: Size<Pixels>) -> bool {
    viewport.width.min(viewport.height) < px(600.)
}
pub fn navigation(phone: bool, cx: &App) -> Div {
    div()
        .debug_selector(|| "navigation".into())
        .flex()
        .items_center()
        .flex_shrink_0()
        .gap_2()
        .when(phone, |nav| {
            nav.w_full().h(px(56.)).justify_around().border_t_1()
        })
        .when(!phone, |nav| {
            nav.flex_col().w(px(52.)).h_full().py_3().border_r_1()
        })
        .border_color(cx.theme().border)
}
pub fn navigation_button(
    id: &'static str,
    label: SharedString,
    icon: gpui_kit::assets::IconName,
    selected: bool,
    phone: bool,
    cx: &App,
) -> Button {
    Button::new(id)
        .ghost()
        .icon(icon)
        .when(phone, |button| button.size(px(44.)))
        .tooltip(label.clone())
        .accessibility_label(label)
        .when(selected, |button| button.bg(cx.theme().accent))
}
pub fn identity(window: &Window) -> Stateful<Div> {
    div()
        .id("account-identity")
        .debug_selector(|| "account-identity".into())
        .w_72()
        .flex_shrink_0()
        .pl(crate::window_chrome::leading_inset(window))
        .pr_3()
        .flex()
        .items_center()
        .gap_2()
}
pub fn conversation_list(phone: bool, cx: &App) -> Div {
    div()
        .debug_selector(|| "chat-list".into())
        .flex()
        .flex_col()
        .when(phone, |list| list.w_full().min_w_0().flex_1())
        .when(!phone, |list| list.w_72())
        .h_full()
        .flex_shrink_0()
        .when(!phone, |list| list.border_r_1())
        .border_color(cx.theme().border)
}
pub fn conversation_row(id: SharedString, selected: bool, cx: &App) -> Button {
    Button::new(id)
        .ghost()
        .selected(selected)
        .when(selected, |button| button.bg(cx.theme().accent))
        .w_full()
        .h(px(72.))
        .p_3()
}
pub fn conversation_row_content(
    avatar: AnyElement,
    title: String,
    preview: String,
    unread: usize,
    cx: &App,
) -> Div {
    div()
        .flex()
        .w_full()
        .items_center()
        .gap_3()
        .child(avatar)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().truncate().child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(preview),
                ),
        )
        .when(unread > 0, |row| {
            row.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().primary)
                    .child(unread.to_string()),
            )
        })
}
pub fn avatar(side: Pixels, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(side)
        .flex_shrink_0()
        .rounded_full()
        .overflow_hidden()
        .bg(cx.theme().muted)
}
pub fn message_lane(outgoing: bool) -> Div {
    div()
        .flex()
        .w_full()
        .gap_3()
        .items_start()
        .when(outgoing, |row| row.flex_row_reverse())
}
pub fn sender_column(outgoing: bool, phone: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap_1()
        .max_w(rems(36.))
        .when(phone, |col| col.flex_1().min_w_0())
        .when(outgoing, |col| col.items_end())
}
pub fn bubble(outgoing: bool, framed: bool, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .rounded_lg()
        .when(framed, |body| {
            body.px_3().py_2().bg(if outgoing {
                cx.theme().accent
            } else {
                cx.theme().muted
            })
        })
}
pub fn conversation_header(phone: bool, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .h(px(if phone { 52. } else { 48. }))
        .flex_shrink_0()
        .gap_2()
        .when(phone, |header| header.px_2())
        .when(!phone, |header| header.px_5())
        .border_b_1()
        .border_color(cx.theme().border)
}
pub fn composer(phone: bool, cx: &App) -> Div {
    div()
        .debug_selector(|| "composer".into())
        .flex()
        .flex_col()
        .flex_shrink_0()
        .border_t_1()
        .border_color(cx.theme().border)
        .when(phone, |composer| composer.p_2().gap_1())
        .when(!phone, |composer| composer.p_3().gap_2())
}

pub struct Fade {
    pub from: f32,
    pub to: f32,
    pub started: std::time::Instant,
    pub generation: u64,
}
impl Fade {
    pub fn value(&self) -> f32 {
        let progress = (self.started.elapsed().as_secs_f32() / 0.08).min(1.);
        self.from + (self.to - self.from) * progress
    }
}

pub fn quote(cx: &App) -> Div {
    div()
        .flex()
        .gap_2()
        .items_center()
        .min_w_0()
        .border_l_2()
        .border_color(cx.theme().primary)
        .pl_2()
        .py_1()
}
