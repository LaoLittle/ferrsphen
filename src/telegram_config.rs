use adapter_telegram::TelegramAdapter;

include!(concat!(env!("OUT_DIR"), "/telegram_credentials.rs"));

pub(crate) fn configure(adapter: TelegramAdapter) -> TelegramAdapter {
    match TELEGRAM_CREDENTIALS {
        Some((api_id, api_hash)) => adapter.with_credentials(api_id, api_hash),
        None => adapter,
    }
}
