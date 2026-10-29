use crate::launcher::{Launcher, Protocol};
use gpui_kit::*;
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
    run_with_adapter(root, None);
}

pub fn run_with_adapter(root: PathBuf, protocol: Option<Protocol>) {
    prepare(&root);
    application().run(move |cx| launch(root, protocol, cx));
}

/// Entry point for the UIKit host in `ios/host.m`, called from
/// `application:didFinishLaunchingWithOptions:` on the main thread.
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn ferrsphen_ios_start() {
    use gpui_mobile::ios::ffi;
    let root = default_data_dir();
    prepare(&root);
    ffi::gpui_ios_initialize();
    // UIKit owns the run loop, so the application state must outlive this call.
    std::mem::forget(application().run_embedded(move |cx| launch(root, None, cx)));
    ffi::gpui_ios_did_finish_launching(std::ptr::null_mut());
}

/// Hand the process to UIKit; `ios/Host.swift` calls back into [`ferrsphen_ios_start`].
#[cfg(target_os = "ios")]
pub fn run_ios() -> ! {
    unsafe extern "C" {
        fn ferrsphen_ios_main() -> std::ffi::c_int;
    }
    std::process::exit(unsafe { ferrsphen_ios_main() })
}

fn prepare(root: &std::path::Path) {
    crate::i18n::init();
    let packet_log = ferrsphen::packet_log::init(root).expect("初始化 packet 日志失败");
    tracing::info!(path = %packet_log.display(), "Packet log");
    let cleanup_root = root.to_path_buf();
    let cleanup = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(crate::storage::purge_legacy_telegram_credentials(
                &cleanup_root,
            ))
    })
    .join();
    match cleanup {
        Ok(Ok(records)) => {
            if records != 0 {
                tracing::info!(records, "Removed legacy Telegram application credentials");
            }
        }
        Ok(Err(error)) => tracing::error!(%error,"Telegram credential cleanup failed"),
        Err(_) => tracing::error!("Telegram credential cleanup task failed"),
    }
}

fn application() -> Application {
    ferrsphen::platform::application().with_assets(gpui_kit::assets::AllAssets)
}

fn launch(root: PathBuf, protocol: Option<Protocol>, cx: &mut App) {
    use gpui_kit::component as gpui_component;
    rust_i18n::extend!(gpui_component);
    gpui_kit::init(cx);
    ferrsphen::window_chrome::init(cx);
    crate::settings::apply(crate::settings::Preferences::default(), cx);
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
    let (handle, _) = gpui_kit::open_window(
        crate::window_chrome::app_options(crate::window_chrome::AppWindowKind::Login, cx),
        cx,
        |window, cx| cx.new(|cx| Launcher::new(root, protocol, window, cx)),
    )
    .expect("window");
    let _ = handle.update(cx, |_, window, _| {
        tracing::info!(viewport=?window.viewport_size(),scale=window.scale_factor(),"Main window created");
        window.on_next_frame(|window, _| {
            tracing::info!(viewport=?window.viewport_size(),"Main window first frame callback");
        });
        window.refresh();
    });
    cx.activate(true);
}

gpui_kit::actions!(ferrsphen, [Quit]);

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("ferrsphen")
            .with_max_level(log::LevelFilter::Info)
            .with_filter(android_logger::FilterBuilder::new().parse("info").build()),
    );
    gpui_mobile::android::host::on_gpu_outcome(|outcome| {
        if outcome == gpui_mobile::android::host::GpuOutcome::Failed {
            tracing::error!(?outcome, "Android renderer initialization failed");
        } else {
            tracing::info!(?outcome, "Android renderer initialized");
        }
    });
    gpui_mobile::android::jni::install_panic_hook();
    let root = app
        .internal_data_path()
        .expect("application data directory")
        .join("ferrsphen");
    gpui_mobile::android::jni::init_platform(&app);
    gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        rustls_platform_verifier::android::init_with_env(env, activity)
            .map_err(|error| error.to_string())
    })
    .expect("initialize Android TLS certificate verifier");
    run(root);
}
