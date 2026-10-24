rust_i18n::i18n!("locales", fallback = "en");

pub mod captcha;
pub mod i18n;
pub mod settings;

extern crate self as ferrsphen;

pub mod backend;
mod image_cache;
pub mod message_content;
pub mod model;
pub mod storage;
pub mod voice;

pub mod card_preview;
#[cfg(feature = "desktop")]
pub mod image_loading;
#[cfg(feature = "desktop")]
pub mod ui_motion;
pub mod viewer;

#[cfg(feature = "desktop")]
pub mod video;

#[cfg(feature = "desktop")]
pub mod audio;

#[cfg(feature = "desktop")]
pub mod window_chrome;

pub mod packet_log;
pub mod search;

#[cfg(feature = "desktop")]
pub mod rich_clipboard;

#[cfg(feature = "desktop")]
pub mod platform;

#[cfg(feature = "desktop")]
pub mod app;
#[cfg(feature = "desktop")]
pub mod view;

pub mod expressions;

#[cfg(feature = "desktop")]
mod message_text;

pub mod activity;

pub mod proto;
