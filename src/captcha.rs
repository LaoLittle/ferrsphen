#[cfg(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
))]
use anyhow::{Context as _, ensure};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[cfg(any(
    test,
    all(
        feature = "desktop",
        not(any(target_os = "android", target_os = "ios"))
    )
))]
const MAX_MESSAGE: usize = 32 * 1024;
#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub url: String,
    pub preferences: crate::settings::Preferences,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub ticket: String,
    pub randstr: String,
}
pub fn trusted_url(value: &str) -> bool {
    reqwest::Url::parse(value).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some_and(|host| {
                host == "qq.com"
                    || host.ends_with(".qq.com")
                    || host == "captcha.qcloud.com"
                    || host.ends_with(".captcha.qcloud.com")
            })
    })
}
#[cfg(any(
    test,
    all(
        feature = "desktop",
        not(any(target_os = "android", target_os = "ios"))
    )
))]
fn parse_ticket(origin: &str, payload: &str) -> Option<Ticket> {
    if !trusted_url(origin) || payload.len() > MAX_MESSAGE {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    if value["kind"] != "captcha" || value["ret"].as_i64() != Some(0) {
        return None;
    }
    let result = Ticket {
        ticket: value["ticket"].as_str()?.into(),
        randstr: value["randstr"].as_str()?.into(),
    };
    (!result.ticket.is_empty()
        && result.ticket.len() <= 16384
        && !result.randstr.is_empty()
        && result.randstr.len() <= 1024)
        .then_some(result)
}

#[cfg(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
))]
pub async fn verify(request: Request) -> Result<Option<Ticket>> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt as _;
    ensure!(trusted_url(&request.url), "Unsupported verification origin");
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("--captcha-window")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().context("Create verification window")?;
    let mut input = child
        .stdin
        .take()
        .context("Verification input unavailable")?;
    input.write_all(&serde_json::to_vec(&request)?).await?;
    input.shutdown().await?;
    drop(input);
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        bail!(
            "Verification window failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(2048)
                .collect::<String>()
        );
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    ensure!(
        output.stdout.len() <= MAX_MESSAGE,
        "Verification result too large"
    );
    let ticket: Ticket =
        serde_json::from_slice(&output.stdout).context("Invalid verification result")?;
    ensure!(
        !ticket.ticket.is_empty()
            && ticket.ticket.len() <= 16384
            && !ticket.randstr.is_empty()
            && ticket.randstr.len() <= 1024,
        "Invalid verification credentials"
    );
    Ok(Some(ticket))
}
#[cfg(not(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
)))]
pub async fn verify(_: Request) -> Result<Option<Ticket>> {
    bail!("Native verification callbacks are unavailable on this platform")
}

#[cfg(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
))]
pub fn run_window() -> Result<()> {
    use gpui_kit::*;
    use std::io::{Read as _, Write as _};
    let mut input = Vec::new();
    std::io::stdin()
        .take((MAX_MESSAGE + 1) as u64)
        .read_to_end(&mut input)?;
    ensure!(input.len() <= MAX_MESSAGE, "Verification input too large");
    let request: Request = serde_json::from_slice(&input)?;
    ensure!(trusted_url(&request.url), "Unsupported verification origin");
    #[cfg(target_os = "windows")]
    // This runs before threads and GPUI start, and only in the verification process.
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "true");
    }
    #[cfg(target_os = "linux")]
    let app = gpui_kit::platform::linux(WindowingModes::X11);
    #[cfg(not(target_os = "linux"))]
    let app = gpui_kit::application();
    app.with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        use gpui_kit::component as gpui_component;
        rust_i18n::extend!(gpui_component);
        gpui_kit::init(cx);
        crate::window_chrome::init(cx);
        crate::settings::apply(request.preferences, cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let result = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(440.), px(560.)), cx)),
                window_min_size: Some(size(px(360.), px(440.))),
                ..crate::window_chrome::options(rust_i18n::t!("login.verification"))
            },
            cx,
            |window, cx| CaptchaWindow::new(request.url, window, cx),
        );
        match result {
            Ok(_) => cx.activate(true),
            Err(error) => {
                eprintln!("Create verification window failed: {error:#}");
                cx.quit();
            }
        }
    });
    std::io::stdout().flush()?;
    Ok(())
}

#[cfg(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
))]
struct CaptchaWindow {
    webview: Option<gpui_kit::Entity<gpui_webview::WebView>>,
    error: Option<String>,
    _result: gpui_kit::Task<()>,
}
#[cfg(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
))]
impl CaptchaWindow {
    fn new(
        url: String,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::App,
    ) -> gpui_kit::Entity<Self> {
        use gpui_kit::*;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let builder = wry::WebViewBuilder::new()
            .with_url(&url)
            .with_initialization_script(include_str!("captcha/bridge.js"))
            .with_ipc_handler(move |request| {
                if let Some(ticket) = parse_ticket(&request.uri().to_string(), request.body()) {
                    let _ = sender.send(ticket);
                }
            })
            .with_navigation_handler(|url| trusted_url(&url))
            .with_download_started_handler(|_, _| false)
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny);
        let result = gpui_webview::WebView::build(builder, window, cx);
        let (webview, error) = match result {
            Ok(webview) => (Some(cx.new(|_| webview)), None),
            Err(error) => {
                eprintln!("Create WebView failed: {error}");
                (
                    None,
                    Some(rust_i18n::t!("login.verification_failed").into_owned()),
                )
            }
        };
        cx.new(|cx: &mut Context<Self>| {
            let task = cx.spawn(async move |this, cx| {
                if let Some(ticket) = receiver.recv().await {
                    let _ = this.update(cx, |this, cx| {
                        this.webview.take();
                        if let Ok(result) = serde_json::to_string(&ticket) {
                            println!("{result}");
                        }
                        cx.quit();
                    });
                }
            });
            Self {
                webview,
                error,
                _result: task,
            }
        })
    }
}
#[cfg(all(
    feature = "desktop",
    not(any(target_os = "android", target_os = "ios"))
))]
impl gpui_kit::Render for CaptchaWindow {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{prelude::FluentBuilder, *};
        crate::window_chrome::surface(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    crate::window_chrome::title_bar(cx)
                        .child(div().flex_1().child(rust_i18n::t!("login.verification"))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .when_some(self.webview.clone(), |view, webview| view.child(webview)),
                )
                .when_some(self.error.clone(), |view, error| {
                    view.child(div().p_4().child(error))
                }),
            window,
            cx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_ticket, trusted_url};
    #[test]
    fn ticket_bridge_accepts_success_only_from_verification_origins() {
        let success = r#"{"kind":"captcha","ret":0,"ticket":"ticket","randstr":"random"}"#;
        assert!(parse_ticket("https://ssl.captcha.qq.com/template", success).is_some());
        for origin in [
            "http://ssl.captcha.qq.com/",
            "https://qq.com.evil.example/",
            "https://user@qq.com/",
            "file:///tmp/page.html",
        ] {
            assert!(!trusted_url(origin));
            assert!(parse_ticket(origin, success).is_none());
        }
        for payload in [
            r#"{"kind":"captcha","ret":2,"ticket":"x","randstr":"y"}"#,
            r#"{"kind":"captcha","ret":0,"ticket":"","randstr":"y"}"#,
            "invalid",
        ] {
            assert!(parse_ticket("https://ssl.captcha.qq.com/", payload).is_none());
        }
    }
}
