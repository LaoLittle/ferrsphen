use crate::{backend::Backend, view};
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
use std::path::PathBuf;

pub fn default_data_dir() -> PathBuf {
    if cfg!(target_os = "ios") {
        PathBuf::from(std::env::var_os("HOME").expect("application container"))
            .join("Library/Application Support/ferrsphen")
    } else {
        dirs::data_local_dir()
            .expect("application data directory")
            .join("ferrsphen")
    }
}

pub fn run(root: PathBuf) {
    crate::i18n::init();
    let packet_log = ferrsphen::packet_log::init(&root).expect("初始化 packet 日志失败");
    tracing::info!(path = %packet_log.display(), "Packet log");
    let mut backend = Backend::start(root);
    let receiver = backend.updates.take().expect("updates");
    let commands = backend.commands.clone();
    ferrsphen::platform::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            let mut backend = Some(backend);
            cx.on_app_quit(move |_| {
                if let Some(backend) = backend.take() {
                    backend.shutdown();
                }
                async {}
            })
            .detach();
            use gpui_kit::component as gpui_component;
            rust_i18n::extend!(gpui_component);
            gpui_kit::init(cx);
            ferrsphen::window_chrome::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            cx.set_window_appearance(Some(WindowAppearance::Dark));
            Theme::update(cx, |theme| {
                theme.colors.background = rgb(0x202020).into();
                theme.colors.muted = rgb(0x303030).into();
                theme.colors.border = rgb(0x343434).into();
                theme.colors.accent = rgb(0x2d3f52).into();
                theme.colors.primary = rgb(0x508be8).into();
            });
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            gpui_kit::open_window(
                crate::window_chrome::app_options(crate::window_chrome::AppWindowKind::Login, cx),
                cx,
                |window, cx| cx.new(|cx| view::AppView::new(commands, receiver, window, cx)),
            )
            .expect("window");
            cx.activate(true);
        });
}

gpui_kit::actions!(ferrsphen, [Quit]);

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(android_logger::Config::default().with_tag("ferrsphen"));
    gpui_mobile::android::jni::install_panic_hook();
    let root = app
        .internal_data_path()
        .expect("application data directory")
        .join("ferrsphen");
    gpui_mobile::android::jni::init_platform(&app);
    run(root);
}
