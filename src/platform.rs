use gpui_kit::Application;

pub fn application() -> Application {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        gpui_mobile::set_system_chrome(&gpui_mobile::SystemChromeStyle {
            status_bar_style: gpui_mobile::StatusBarContentStyle::Light,
            ..Default::default()
        });
        #[cfg(target_os = "ios")]
        let platform = gpui_mobile::current_platform(false);
        #[cfg(target_os = "android")]
        let platform = std::rc::Rc::new(
            gpui_mobile::android::jni::shared_platform().expect("Android platform initialized"),
        );
        Application::with_platform(platform)
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        gpui_kit::application()
    }
}
