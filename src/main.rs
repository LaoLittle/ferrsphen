#[cfg(not(target_os = "ios"))]
use std::path::PathBuf;

#[cfg(not(target_os = "ios"))]
fn main() {
    let mut root = ferrsphen::app::default_data_dir();
    let mut protocol = None;
    let mut phone = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        match argument.to_str() {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            Some("--captcha-window") => {
                if let Err(error) = ferrsphen::captcha::run_window() {
                    eprintln!("{error:#}");
                    std::process::exit(1);
                }
                return;
            }
            Some("--adapter") => {
                protocol = Some(
                    args.next()
                        .and_then(|value| {
                            value
                                .to_str()
                                .and_then(ferrsphen::launcher::Protocol::parse)
                        })
                        .expect("--adapter qq|telegram|matrix|mock"),
                );
            }
            Some("--data-dir") => root = args.next().map(PathBuf::from).expect("--data-dir PATH"),
            Some("--phone") => phone = true,
            Some("--help" | "-h") => {
                println!(
                    "ferrsphen [--data-dir PATH] [--adapter qq|telegram|matrix|mock] [--phone]"
                );
                println!("  --phone  以手机布局和尺寸打开桌面窗口；未指定 --adapter 时使用 mock");
                return;
            }
            _ => {
                eprintln!("未知参数：{}", argument.to_string_lossy());
                std::process::exit(2);
            }
        }
    }
    if phone {
        ferrsphen::platform::simulate_phone();
        protocol = protocol.or(Some(ferrsphen::launcher::Protocol::Mock));
    }
    ferrsphen::app::run_with_adapter(root, protocol);
}

#[cfg(target_os = "ios")]
fn main() {
    ferrsphen::app::run_ios();
}
