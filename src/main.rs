mod view;
use ferrsphen::backend::Backend;
use gpui_kit::{
    component::{Theme, ThemeMode},
    *,
};
use std::path::PathBuf;
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--data-dir") => root = args.next().map(PathBuf::from).expect("--data-dir PATH"),
            Some("--help" | "-h") => {
                println!("ferrsphen [--data-dir PATH]");
                return;
            }
            _ => {
                eprintln!("未知参数：{}", argument.to_string_lossy());
                std::process::exit(2);
            }
        }
    }
    let mut backend = Backend::start(root);
    let receiver = backend.updates.take().expect("updates");
    let commands = backend.commands.clone();
    gpui_kit::application()
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
            gpui_kit::init(cx);
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
                WindowOptions {
                    window_bounds: Some(WindowBounds::centered(size(px(1240.), px(820.)), cx)),
                    app_owns_titlebar_drag: true,
                    window_min_size: Some(size(px(880.), px(580.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("ferrsphen".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(12.), px(20.))),
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| view::AppView::new(commands, receiver, window, cx)),
            )
            .expect("window");
            cx.activate(true);
        });
}
gpui_kit::actions!(ferrsphen, [Quit]);
