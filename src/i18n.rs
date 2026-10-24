#[cfg(feature = "desktop")]
pub use gpui_kit::component::{locale, set_locale};
#[cfg(not(feature = "desktop"))]
pub use rust_i18n::{locale, set_locale};

pub fn resolve(locale: &str) -> &'static str {
    let language = locale
        .split('.')
        .next()
        .unwrap_or(locale)
        .replace('_', "-")
        .to_ascii_lowercase();
    if language == "zh" || language.starts_with("zh-") {
        "zh-CN"
    } else if language == "ja" || language.starts_with("ja-") {
        "ja"
    } else if language == "ko" || language.starts_with("ko-") {
        "ko"
    } else {
        "en"
    }
}

pub fn system_locale() -> &'static str {
    let language = std::env::var("FERRSPHEN_LANGUAGE")
        .ok()
        .or_else(sys_locale::get_locale)
        .unwrap_or_else(|| "en".into());
    resolve(&language)
}

pub fn init() {
    set_locale(system_locale());
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
        for (locale, copy, select_all) in
            [("ja", "コピー", "すべて選択"), ("ko", "복사", "모두 선택")]
        {
            assert_eq!(
                gpui_component::_rust_i18n_translate(locale, "Input.Copy"),
                copy
            );
            assert_eq!(
                gpui_component::_rust_i18n_translate(locale, "Input.Select All"),
                select_all
            );
        }
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

#[cfg(test)]
mod catalog_tests {
    use std::collections::BTreeMap;
    fn catalog(text: &str) -> BTreeMap<&str, String> {
        text.lines()
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key, serde_json::from_str(value.trim()).unwrap()))
            .collect()
    }
    fn placeholders(text: &str) -> Vec<&str> {
        let mut values = text
            .split("%{")
            .skip(1)
            .map(|s| s.split('}').next().unwrap())
            .collect::<Vec<_>>();
        values.sort();
        values
    }
    #[test]
    fn japanese_and_korean_cover_every_label_and_preserve_interpolation() {
        let english = catalog(include_str!("../locales/en.yml"));
        for (locale, text) in [
            ("ja", include_str!("../locales/ja.yml")),
            ("ko", include_str!("../locales/ko.yml")),
        ] {
            let translated = catalog(text);
            for (key, value) in &english {
                let actual = translated
                    .get(key)
                    .unwrap_or_else(|| panic!("missing {locale}: {key}"));
                assert_eq!(placeholders(value), placeholders(actual), "{locale}: {key}");
                assert_eq!(rust_i18n::t!(*key, locale = locale).as_ref(), actual);
            }
        }
        assert_eq!(super::resolve("ja_JP.UTF-8"), "ja");
        assert_eq!(super::resolve("ko-KR"), "ko");
        assert_eq!(
            rust_i18n::t!("forward.count", locale = "ja", count = 12),
            "12 件のメッセージを表示"
        );
        assert_eq!(
            rust_i18n::t!("forward.count", locale = "ko", count = 12),
            "메시지 12개 보기"
        );
    }
}
