use serde::{Deserialize, Serialize};

pub const STORAGE_KEY: &str = "preferences/ui";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    System,
    Chinese,
    English,
    Japanese,
    Korean,
}
impl Language {
    pub const ALL: [Self; 5] = [
        Self::System,
        Self::Chinese,
        Self::English,
        Self::Japanese,
        Self::Korean,
    ];
    pub fn native_name(self) -> &'static str {
        match self {
            Self::System => "",
            Self::Chinese => "中文",
            Self::English => "English",
            Self::Japanese => "日本語",
            Self::Korean => "한국어",
        }
    }
    pub fn locale(self) -> &'static str {
        match self {
            Self::System => crate::i18n::system_locale(),
            Self::Chinese => "zh-CN",
            Self::English => "en",
            Self::Japanese => "ja",
            Self::Korean => "ko",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "language-system",
            Self::Chinese => "language-zh",
            Self::English => "language-en",
            Self::Japanese => "language-ja",
            Self::Korean => "language-ko",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}
impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
    pub fn label_key(self) -> &'static str {
        match self {
            Self::System => "settings.system",
            Self::Light => "settings.light",
            Self::Dark => "settings.dark",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::System => "appearance-system",
            Self::Light => "appearance-light",
            Self::Dark => "appearance-dark",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub language: Language,
    pub appearance: Appearance,
}

#[cfg(feature = "desktop")]
pub fn apply(preferences: Preferences, cx: &mut gpui_kit::App) {
    use gpui_kit::WindowAppearance;
    crate::i18n::set_locale(preferences.language.locale());
    cx.set_window_appearance(match preferences.appearance {
        Appearance::System => None,
        Appearance::Light => Some(WindowAppearance::Light),
        Appearance::Dark => Some(WindowAppearance::Dark),
    });
    apply_theme(preferences.appearance, cx.window_appearance(), cx);
    cx.refresh_windows();
}

#[cfg(feature = "desktop")]
pub fn apply_theme(
    appearance: Appearance,
    system: gpui_kit::WindowAppearance,
    cx: &mut gpui_kit::App,
) {
    use gpui_kit::{
        component::{Theme, ThemeMode},
        rgb,
    };
    let mode = match appearance {
        Appearance::System => system.into(),
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
    };
    Theme::change(mode, None, cx);
    Theme::update(cx, |theme| {
        let (background, muted, border, accent, primary) = if mode == ThemeMode::Dark {
            (0x202020, 0x303030, 0x343434, 0x2d3f52, 0x508be8)
        } else {
            (0xf7f7f7, 0xededed, 0xdedede, 0xdce9f8, 0x3478d6)
        };
        theme.colors.background = rgb(background).into();
        theme.colors.muted = rgb(muted).into();
        theme.colors.border = rgb(border).into();
        theme.colors.accent = rgb(accent).into();
        theme.colors.primary = rgb(primary).into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn preferences_survive_database_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let db = crate::storage::Database::open(directory.path()).unwrap();
        let preferences = Preferences {
            language: Language::Korean,
            appearance: Appearance::Light,
        };
        db.put(STORAGE_KEY, &preferences).await.unwrap();
        db.close().await.unwrap();
        drop(db);
        let db = crate::storage::Database::open(directory.path()).unwrap();
        assert_eq!(
            db.get::<Preferences>(STORAGE_KEY).unwrap(),
            Some(preferences)
        );
        db.close().await.unwrap();
    }
}
