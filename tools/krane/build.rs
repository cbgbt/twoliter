use std::env;
use std::path::PathBuf;
use std::process::Command;

const REQUIRED_TOOLS: &[&str] = &["go"];
const CFLAGS: &str = concat!(
    "-O2 -g -Wformat -Werror=format-security -Wp,-D_FORTIFY_SOURCE=2 -Wp,-D_GLIBCXX_ASSERTIONS ",
    "-fexceptions -fstack-clash-protection -fno-omit-frame-pointer",
);
const LDFLAGS: &str = "-Wl,-z,relro -Wl,-z,now";

fn main() {
    let script_dir = env::current_dir().unwrap();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo::rerun-if-changed=go-src");

    ensure_required_tools_installed();

    // build krane FFI wrapper
    let build_output_loc = out_dir.join("libkrane.a");
    let mut build_command = Command::new("go");

    let curr_cflags = env::var("CFLAGS").unwrap_or_default();
    let desired_cflags = format!("{curr_cflags} {CFLAGS}");

    let curr_ldflags = env::var("LDFLAGS").unwrap_or_default();
    let desired_ldflags = format!("{curr_ldflags} {LDFLAGS}");

    build_command
        .env("GOOS", get_goos())
        .env("GOARCH", get_goarch())
        .env("CGO_ENABLED", "1")
        .env("CFLAGS", &desired_cflags)
        .env("CGO_CFLAGS", &desired_cflags)
        .env("CXXFLAGS", &desired_cflags)
        .env("CGO_CXXFLAGS", &desired_cflags)
        .env("LDFLAGS", &desired_ldflags)
        .env("CGO_LDFLAGS", &desired_ldflags)
        .arg("build")
        .arg("-buildmode=c-archive")
        .arg("-o")
        .arg(&build_output_loc)
        .arg("main.go")
        .current_dir(script_dir.join("go-src"));

    // Set cross-compiler when using cargo-cross
    let cross_cc_var = format!("CC_{}", env::var("TARGET").unwrap().replace("-", "_"));
    if let Some(cross_cc) = env::var_os(&cross_cc_var) {
        build_command.env("CC", cross_cc);
    }
    let cross_ar_var = format!("AR_{}", env::var("TARGET").unwrap().replace("-", "_"));
    if let Some(cross_ar) = env::var_os(&cross_ar_var) {
        build_command.env("AR", cross_ar);
    }

    let exit_status = build_command.status().expect("Failed to build crane");

    assert!(
        exit_status.success(),
        "Failed to build krane -- go compiler exited nonzero"
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=krane");
}

fn ensure_required_tools_installed() {
    for tool in REQUIRED_TOOLS {
        which::which(tool)
            .unwrap_or_else(|_| panic!("Must have the `{tool}` utility installed in PATH"));
    }
}

fn get_goos() -> &'static str {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Failed to read CARGO_CFG_TARGET_OS");
    match target_os.as_str() {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "darwin",
        other => panic!("Unsupported target OS: {}", other),
    }
}

fn get_goarch() -> &'static str {
    let target_arch =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Failed to read CARGO_CFG_TARGET_ARCH");

    match target_arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "wasm32" => "wasm",
        other => panic!("Unsupported target architecture: {}", other),
    }
}
