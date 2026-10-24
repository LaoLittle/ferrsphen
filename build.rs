fn main() {
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
