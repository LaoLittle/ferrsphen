fn main() {
    println!("cargo:rerun-if-changed=secrets.toml");
    println!("cargo:rerun-if-env-changed=TELEGRAM_API_ID");
    println!("cargo:rerun-if-env-changed=TELEGRAM_API_HASH");
    let api_id = std::env::var("TELEGRAM_API_ID").ok();
    let api_hash = std::env::var("TELEGRAM_API_HASH").ok();
    let environment = [&api_id, &api_hash]
        .iter()
        .all(|value| value.as_ref().is_some_and(|value| !value.trim().is_empty()));
    let local = if environment {
        None
    } else {
        match std::fs::read_to_string("secrets.toml") {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => panic!("Unable to read secrets.toml"),
        }
    };
    let credentials = resolve(api_id.as_deref(), api_hash.as_deref(), local.as_deref())
        .unwrap_or_else(|error| panic!("{error}"));
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(
        output.join("telegram_credentials.rs"),
        source(credentials.as_ref()),
    )
    .expect("Write Telegram build configuration");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64")
    {
        // rav1d assembly references exported Rust tables with PC-relative relocations.
        // Bind definitions locally inside the cdylib to prevent symbol preemption.
        println!("cargo:rustc-cdylib-link-arg=-Wl,-Bsymbolic");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        swift_host();
    }
    println!("cargo:rerun-if-changed=src/proto");
    println!("cargo:rerun-if-changed=locales");
    let mut config = prost_build::Config::new();
    config.skip_debug([
        ".ferrsphen.session.Keystore",
        ".ferrsphen.session.Session",
        ".ferrsphen.session.KeystoreFile",
        ".ferrsphen.storage.Record",
    ]);
    config.include_file("generated.rs");
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path().expect("protoc"));
    let mut schemas: Vec<_> = std::fs::read_dir("src/proto")
        .expect("protobuf schema directory")
        .map(|entry| entry.expect("protobuf schema").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "proto")
        })
        .collect();
    schemas.sort();
    config
        .compile_protos(&schemas, &["src/proto"])
        .expect("compile protobuf schemas");
}

fn swift_host() {
    use std::process::Command;
    println!("cargo:rerun-if-changed=ios/Host.swift");
    println!("cargo:rerun-if-changed=ios/FerrsphenHost.h");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
    let target = std::env::var("TARGET").expect("TARGET");
    let simulator = target.ends_with("-sim") || target.starts_with("x86_64");
    let (sdk, suffix) = if simulator {
        ("iphonesimulator", "-simulator")
    } else {
        ("iphoneos", "")
    };
    let arch = if target.starts_with("x86_64") { "x86_64" } else { "arm64" };
    let deployment = std::env::var("IPHONEOS_DEPLOYMENT_TARGET").unwrap_or_else(|_| "16.0".into());
    let xcrun = |arguments: &[&str]| {
        let output = Command::new("xcrun")
            .args(arguments)
            .output()
            .expect("run xcrun");
        assert!(output.status.success(), "xcrun {arguments:?} failed");
        String::from_utf8(output.stdout).expect("xcrun output").trim().to_owned()
    };
    let sdk_path = xcrun(&["--sdk", sdk, "--show-sdk-path"]);
    let swiftc = std::path::PathBuf::from(xcrun(&["--sdk", sdk, "--find", "swiftc"]));
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let status = Command::new(&swiftc)
        .args(["-parse-as-library", "-emit-library", "-static", "-module-name", "FerrsphenHost"])
        .args(["-target", &format!("{arch}-apple-ios{deployment}{suffix}")])
        .args(["-sdk", &sdk_path])
        .args(["-import-objc-header", "ios/FerrsphenHost.h"])
        .arg(if release { "-O" } else { "-Onone" })
        .arg("-o")
        .arg(output.join("libferrsphen_ios_host.a"))
        .arg("ios/Host.swift")
        .status()
        .expect("run swiftc");
    assert!(status.success(), "swiftc failed");
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=ferrsphen_ios_host");
    // Swift objects auto-link the runtime; point the linker at the SDK stubs and the
    // toolchain's back-deployment compatibility libraries.
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    let toolchain = swiftc.parent().and_then(|bin| bin.parent()).expect("toolchain");
    println!(
        "cargo:rustc-link-search=native={}",
        toolchain.join("lib/swift").join(sdk).display()
    );
}

// Telegram application credentials.
struct Credentials {
    api_id: i32,
    api_hash: String,
}

fn pair(api_id: &str, api_hash: &str) -> Result<Credentials, &'static str> {
    let api_id = api_id
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or("Telegram API ID must be a positive 32-bit integer")?;
    let api_hash = api_hash.trim();
    if api_hash.len() != 32 || !api_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Telegram API Hash must contain 32 hexadecimal characters");
    }
    Ok(Credentials {
        api_id,
        api_hash: api_hash.to_owned(),
    })
}

fn resolve(
    api_id: Option<&str>,
    api_hash: Option<&str>,
    local: Option<&str>,
) -> Result<Option<Credentials>, &'static str> {
    let present = |value: &str| !value.trim().is_empty();
    if let (Some(id), Some(hash)) = (
        api_id.filter(|value| present(value)),
        api_hash.filter(|value| present(value)),
    ) {
        return pair(id, hash).map(Some);
    }
    let Some(local) = local else { return Ok(None) };
    let config: toml::Value = toml::from_str(local).map_err(|_| "Invalid secrets.toml syntax")?;
    let Some(telegram) = config.get("telegram") else {
        return Ok(None);
    };
    let telegram = telegram
        .as_table()
        .ok_or("secrets.toml requires a [telegram] table")?;
    let Some(id) = telegram.get("api_id") else {
        return Ok(None);
    };
    let id = match id {
        toml::Value::Integer(id) => id.to_string(),
        toml::Value::String(id) => id.clone(),
        _ => return Err("telegram.api_id must be an integer"),
    };
    let Some(hash) = telegram.get("api_hash") else {
        return Ok(None);
    };
    let hash = hash.as_str().ok_or("telegram.api_hash must be a string")?;
    if hash.trim().is_empty() {
        return Ok(None);
    }
    pair(&id, hash).map(Some)
}

fn source(credentials: Option<&Credentials>) -> String {
    let value = credentials.map_or_else(
        || "None".to_owned(),
        |credentials| format!("Some(({}, {:?}))", credentials.api_id, credentials.api_hash),
    );
    format!("pub(super) const TELEGRAM_CREDENTIALS: Option<(i32, &str)> = {value};\n")
}
