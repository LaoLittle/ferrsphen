#[cfg(not(target_os = "ios"))]
use std::path::PathBuf;

#[cfg(not(target_os = "ios"))]
fn main() {
    let mut root = ferrsphen::app::default_data_dir();
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
    ferrsphen::app::run(root);
}

#[cfg(target_os = "ios")]
fn main() {
    ferrsphen::app::run(ferrsphen::app::default_data_dir());
}
