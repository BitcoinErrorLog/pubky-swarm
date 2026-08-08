//! Builds the pinned vendored libtorrent archive and the private CXX facade.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXPECTED_VERSION: &str = "2.0.13.0";

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR"),
    );
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("crate must remain under <workspace>/crates")
        .to_path_buf();
    let vendor_dir = workspace_root.join("vendor/libtorrent");
    let version_header = vendor_dir.join("include/libtorrent/version.hpp");

    verify_vendor_source(&version_header);

    println!("cargo:rerun-if-changed={}", version_header.display());
    println!(
        "cargo:rerun-if-changed={}",
        vendor_dir.join("CMakeLists.txt").display()
    );
    println!("cargo:rerun-if-changed=src/ffi.rs");
    println!("cargo:rerun-if-changed=../src/facade.cc");
    println!("cargo:rerun-if-changed=../include/facade.hpp");
    for variable in [
        "BOOST_ROOT",
        "OPENSSL_ROOT_DIR",
        "OPENSSL_LIB_DIR",
        "CMAKE_TOOLCHAIN_FILE",
        "CC",
        "CXX",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    let target = env::var("TARGET").expect("Cargo must set TARGET");
    let native = native_dependencies(&target);
    let install_dir = compile_libtorrent(&vendor_dir, &native);
    compile_facade(&manifest_dir, &install_dir, &native);
    emit_link_directives(&target, &install_dir, &native);
}

struct NativeDependencies {
    boost_root: Option<PathBuf>,
    openssl_root: Option<PathBuf>,
}

fn native_dependencies(target: &str) -> NativeDependencies {
    let boost_root = env::var_os("BOOST_ROOT").map(PathBuf::from);
    let openssl_root = env::var_os("OPENSSL_ROOT_DIR").map(PathBuf::from);

    if target.contains("apple-darwin") {
        NativeDependencies {
            boost_root: boost_root.or_else(|| brew_prefix("boost")),
            openssl_root: openssl_root.or_else(|| brew_prefix("openssl@3")),
        }
    } else {
        NativeDependencies {
            boost_root,
            openssl_root,
        }
    }
}

fn brew_prefix(formula: &str) -> Option<PathBuf> {
    let output = Command::new("brew")
        .args(["--prefix", formula])
        .output()
        .unwrap_or_else(|error| {
            panic!("failed to run `brew --prefix {formula}`: {error}");
        });
    assert!(
        output.status.success(),
        "Homebrew formula `{formula}` is required; install it or set its root explicitly"
    );
    let path = String::from_utf8(output.stdout)
        .expect("Homebrew prefix must be UTF-8")
        .trim()
        .to_owned();
    Some(PathBuf::from(path))
}

fn verify_vendor_source(version_header: &Path) {
    let contents = std::fs::read_to_string(version_header).unwrap_or_else(|error| {
        panic!(
            "pinned libtorrent source is missing at {}: {error}; run crates/libtorrent-engine/scripts/fetch-libtorrent.sh",
            version_header.display()
        );
    });
    assert!(
        contents.contains(&format!(
            "#define LIBTORRENT_VERSION \"{EXPECTED_VERSION}\""
        )),
        "vendored source is not libtorrent {EXPECTED_VERSION}"
    );
}

fn compile_libtorrent(vendor_dir: &Path, native: &NativeDependencies) -> PathBuf {
    let mut config = cmake::Config::new(vendor_dir);
    config
        .profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("build_tests", "OFF")
        .define("build_examples", "OFF")
        .define("build_tools", "OFF")
        .define("python-bindings", "OFF")
        .define("deprecated-functions", "OFF")
        .define("dht", "ON")
        .define("encryption", "ON")
        .define("exceptions", "ON")
        .define("extensions", "ON")
        .define("i2p", "OFF")
        .define("logging", "OFF")
        .define("mutable-torrents", "ON")
        .define("streaming", "ON");

    let mut prefix_paths = Vec::new();
    if let Some(boost_root) = &native.boost_root {
        config.define("Boost_ROOT", boost_root);
        prefix_paths.push(boost_root.display().to_string());
    }
    if let Some(openssl_root) = &native.openssl_root {
        config.define("OPENSSL_ROOT_DIR", openssl_root);
        prefix_paths.push(openssl_root.display().to_string());
    }
    if !prefix_paths.is_empty() {
        config.define("CMAKE_PREFIX_PATH", prefix_paths.join(";"));
    }
    if let Some(toolchain) = env::var_os("CMAKE_TOOLCHAIN_FILE") {
        config.define("CMAKE_TOOLCHAIN_FILE", toolchain);
    }

    config.build()
}

fn compile_facade(manifest_dir: &Path, install_dir: &Path, native: &NativeDependencies) {
    let mut build = cxx_build::bridge("src/ffi.rs");
    build
        .file("../src/facade.cc")
        .include(manifest_dir.join("../include"))
        .include(install_dir.join("include"))
        .define("BOOST_ASIO_ENABLE_CANCELIO", None)
        .define("BOOST_ASIO_NO_DEPRECATED", None)
        .define("TORRENT_NO_DEPRECATE", None)
        .define("TORRENT_DISABLE_LOGGING", None)
        .define("TORRENT_USE_I2P", Some("0"))
        .define("TORRENT_USE_OPENSSL", None)
        .define("TORRENT_USE_LIBCRYPTO", None)
        .define("TORRENT_SSL_PEERS", None)
        .define("PUBKY_LIBTORRENT_STATIC", Some("1"))
        .flag_if_supported("-std=c++17");

    if let Some(boost_root) = &native.boost_root {
        build.include(boost_root.join("include"));
    }
    if let Some(openssl_root) = &native.openssl_root {
        build.include(openssl_root.join("include"));
    }

    build.compile("pubky_libtorrent_facade");
}

fn emit_link_directives(target: &str, install_dir: &Path, native: &NativeDependencies) {
    for directory in ["lib", "lib64"] {
        let path = install_dir.join(directory);
        if path.is_dir() {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }
    println!("cargo:rustc-link-lib=static=torrent-rasterbar");

    if let Some(path) = env::var_os("OPENSSL_LIB_DIR").map(PathBuf::from) {
        println!("cargo:rustc-link-search=native={}", path.display());
    } else if let Some(root) = &native.openssl_root {
        println!(
            "cargo:rustc-link-search=native={}",
            root.join("lib").display()
        );
    }

    if target.contains("msvc") {
        println!("cargo:rustc-link-lib=libssl");
        println!("cargo:rustc-link-lib=libcrypto");
        for library in [
            "bcrypt", "mswsock", "ws2_32", "iphlpapi", "dbghelp", "crypt32",
        ] {
            println!("cargo:rustc-link-lib={library}");
        }
    } else {
        println!("cargo:rustc-link-lib=ssl");
        println!("cargo:rustc-link-lib=crypto");
    }

    if target.contains("apple-darwin") {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=SystemConfiguration");
    }
}
