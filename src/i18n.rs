#[cfg(feature = "desktop")]
pub use gpui_kit::component::{locale, set_locale};
#[cfg(not(feature = "desktop"))]
pub use rust_i18n::{locale, set_locale};

pub fn resolve(locale: &str) -> &'static str {
    let language = locale.replace('_', "-").to_ascii_lowercase();
    if language == "zh" || language.starts_with("zh-") {
        "zh-CN"
    } else {
        "en"
    }
}

pub fn init() {
    let language = std::env::var("FERRSPHEN_LANGUAGE")
        .ok()
        .or_else(sys_locale::get_locale)
        .unwrap_or_else(|| "en".into());
    set_locale(resolve(&language));
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "desktop")]
    #[test]
    fn application_catalog_extends_component_strings_and_keeps_builtin_fallbacks() {
        use gpui_kit::component as gpui_component;
        rust_i18n::extend!(gpui_component);
        assert_eq!(
            gpui_component::_rust_i18n_translate("zh-CN", "Input.Clear"),
            "清空"
        );
        assert_eq!(
            gpui_component::_rust_i18n_translate("zh-CN", "Input.Copy"),
            "复制"
        );
        assert_eq!(rust_i18n::t!("common.reply", locale = "en"), "Reply");
    }
    #[test]
    fn normalizes_system_language_tags() {
        for language in ["zh", "zh_CN.UTF-8", "zh-Hans-CN", "zh_TW"] {
            assert_eq!(super::resolve(language), "zh-CN");
        }
        for language in ["en-US", "de_DE", "C", ""] {
            assert_eq!(super::resolve(language), "en");
        }
    }

    #[test]
    fn both_catalogs_translate_labels_and_interpolated_content() {
        for (locale, reply, count) in [
            ("en", "Reply", "View 12 messages"),
            ("zh-CN", "回复", "查看 12 条消息"),
        ] {
            assert_eq!(rust_i18n::t!("common.reply", locale = locale), reply);
            assert_eq!(
                rust_i18n::t!("forward.count", locale = locale, count = 12),
                count
            );
        }
    }
}
