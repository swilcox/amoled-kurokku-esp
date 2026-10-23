use std::process::Command;

#[path = "build_support/fontgen.rs"]
mod fontgen;

fn main() {
    // Build-script cfg evaluates for the *host*, so gate on the target via
    // CARGO_CFG_TARGET_OS instead. On host `cargo test` this is a no-op.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        embuild::espidf::sysenv::output();
    }

    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let suffix = if dirty { "-dirty" } else { "" };
    println!("cargo:rustc-env=KUROKKU_GIT_HASH={}{}", git_hash, suffix);

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    // Fallback wall-clock time for a device whose RTC has never been set
    // (until NTP is available). Re-emitted whenever src/ changes, so it is
    // roughly "when this firmware was built".
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=KUROKKU_BUILD_UNIX_TIME={now}");
    println!("cargo:rerun-if-changed=src");

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    println!("cargo:rerun-if-changed=build_support");
    fontgen::generate(
        std::path::Path::new(&manifest_dir),
        std::path::Path::new(&out_dir),
        &[
            fontgen::FontSpec {
                name: "CLOCK_BIG",
                file: "assets/fonts/BebasNeue-Regular.ttf",
                px: 250.0,
                chars: "0123456789:",
            },
            fontgen::FontSpec {
                name: "CLOCK_DATE",
                file: "assets/fonts/BebasNeue-Regular.ttf",
                px: 48.0,
                chars: " ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            },
            fontgen::FontSpec {
                name: "CLOCK_SECS",
                file: "assets/fonts/BebasNeue-Regular.ttf",
                px: 64.0,
                chars: " 0123456789APM",
            },
        ],
    );

    // Keep partitions.csv in sync with esp-idf-sys's cmake build dirs.
    // The cmake build resolves CONFIG_PARTITION_TABLE_CUSTOM_FILENAME relative
    // to its own source dir (out/), not the project root, so we mirror it there.
    // This runs after esp-idf-sys's cmake build completes, ensuring the file is
    // present for the next incremental build. The Justfile handles bootstrap
    // (first build) by retrying after the initial cmake failure creates out/.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("espidf") {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let src = std::path::Path::new(&manifest_dir).join("partitions.csv");
        // OUT_DIR = .../build/<pkg>-<hash>/out or .../build/<pkg>/<hash>/out
        let build_dir = std::path::Path::new(&out_dir)
            .ancestors()
            .find(|p| p.file_name().is_some_and(|n| n == "build"));
        if let Some(build_dir) = build_dir {
            // Cargo's build-dir layout is build/esp-idf-sys-<hash>/out on older
            // toolchains and build/esp-idf-sys/<hash>/out on newer ones.
            let mut out_dirs = Vec::new();
            for entry in std::fs::read_dir(build_dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("esp-idf-sys-") {
                    out_dirs.push(entry.path().join("out"));
                } else if name == "esp-idf-sys" {
                    for hash in std::fs::read_dir(entry.path()).into_iter().flatten().flatten() {
                        out_dirs.push(hash.path().join("out"));
                    }
                }
            }
            for dir in out_dirs.into_iter().filter(|d| d.is_dir()) {
                let _ = std::fs::copy(&src, dir.join("partitions.csv"));
            }
        }
        println!("cargo:rerun-if-changed=partitions.csv");
    }
}
