#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]
use std::process::Command;

fn main() {
    #[cfg(target_os = "linux")]
    {
        // Add rpaths for libraries that webrtc-sys dlopens at runtime.
        // This is mostly required for hosts with non-standard SO installation
        // locations such as NixOS.
        let dlopened_libs = ["libva", "libva-drm", "egl"];

        let mut rpath_dirs = std::collections::BTreeSet::new();
        for lib in &dlopened_libs {
            if let Some(libdir) = pkg_config::get_variable(lib, "libdir").ok() {
                rpath_dirs.insert(libdir);
            } else {
                eprintln!("zed build.rs: {lib} not found in pkg-config's path");
            }
        }

        for dir in &rpath_dirs {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
        }
    }

    if cfg!(target_os = "macos") {
        println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=10.15.7");

        // Weakly link ReplayKit to ensure Zed can be used on macOS 10.15+.
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ReplayKit");

        // Seems to be required to enable Swift concurrency
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

        // Register exported Objective-C selectors, protocols, etc
        println!("cargo:rustc-link-arg=-Wl,-ObjC");

        // weak link to support Catalina
        println!("cargo:rustc-link-arg=-Wl,-weak_framework,ScreenCaptureKit");
    }

    emit_zednotes_pin();

    // Populate git sha environment variable if git is available
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );

    let git_sha = match std::env::var("ZED_COMMIT_SHA").ok() {
        Some(git_sha) => {
            // In deterministic build environments such as Nix, we inject the commit sha into the build script.
            Some(git_sha)
        }
        None => {
            if let Some(output) = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                && output.status.success()
            {
                let git_sha = String::from_utf8_lossy(&output.stdout);
                Some(git_sha.trim().to_string())
            } else {
                None
            }
        }
    };

    if let Some(git_sha) = git_sha {
        println!("cargo:rustc-env=ZED_COMMIT_SHA={git_sha}");

        if let Some(build_identifier) = option_env!("GITHUB_RUN_NUMBER") {
            println!("cargo:rustc-env=ZED_BUILD_ID={build_identifier}");
        }

        if let Ok(build_profile) = std::env::var("PROFILE")
            && build_profile == "release"
        {
            // This is currently the best way to make `cargo build ...`'s build script
            // to print something to stdout without extra verbosity.
            println!("cargo::warning=Info: using '{git_sha}' hash for ZED_COMMIT_SHA env var");
        }
    }

    if cfg!(windows) {
        if cfg!(target_env = "msvc") {
            // todo(windows): This is to avoid stack overflow. Remove it when solved.
            println!("cargo:rustc-link-arg=/stack:{}", 8 * 1024 * 1024);
            println!("cargo:rustc-link-arg=/DELAYLOAD:windowsperformancerecordercontrol");
            println!("cargo:rustc-link-lib=delayimp");
        }

        if cfg!(target_arch = "x86_64") || cfg!(target_arch = "aarch64") {
            let out_dir = std::env::var("OUT_DIR").unwrap();
            let out_dir: &std::path::Path = out_dir.as_ref();
            let target_dir = std::path::Path::new(&out_dir)
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .expect("Failed to find target directory");

            let conpty_dll_target = target_dir.join("conpty.dll");
            let open_console_target = target_dir.join("OpenConsole.exe");

            let conpty_url = "https://github.com/microsoft/terminal/releases/download/v1.24.10621.0/Microsoft.Windows.Console.ConPTY.1.24.260303001.nupkg";
            let nupkg_path = out_dir.join("conpty.nupkg.zip");
            let extract_dir = out_dir.join("conpty");

            let download_script = format!(
                "$ProgressPreference = 'SilentlyContinue'; [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -Uri '{}' -OutFile '{}'",
                conpty_url,
                nupkg_path.display()
            );

            let download_result = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &download_script,
                ])
                .output();

            match download_result {
                Ok(output) if output.status.success() => {
                    println!("Downloaded conpty nupkg successfully");

                    let extract_script = format!(
                        "$ProgressPreference = 'SilentlyContinue'; Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                        nupkg_path.display(),
                        extract_dir.display()
                    );

                    let extract_result = Command::new("powershell")
                        .args(["-NoProfile", "-NonInteractive", "-Command", &extract_script])
                        .output();

                    match extract_result {
                        Ok(output) if output.status.success() => {
                            let (conpty_dll_source, open_console_source) =
                                if cfg!(target_arch = "x86_64") {
                                    (
                                        extract_dir.join("runtimes/win-x64/native/conpty.dll"),
                                        extract_dir
                                            .join("build/native/runtimes/x64/OpenConsole.exe"),
                                    )
                                } else {
                                    (
                                        extract_dir.join("runtimes/win-arm64/native/conpty.dll"),
                                        extract_dir
                                            .join("build/native/runtimes/arm64/OpenConsole.exe"),
                                    )
                                };

                            match std::fs::copy(&conpty_dll_source, &conpty_dll_target) {
                                Ok(_) => {
                                    println!("Copied conpty.dll to {}", conpty_dll_target.display())
                                }
                                Err(e) => println!(
                                    "cargo::warning=Failed to copy conpty.dll from {}: {}",
                                    conpty_dll_source.display(),
                                    e
                                ),
                            }

                            match std::fs::copy(&open_console_source, &open_console_target) {
                                Ok(_) => println!(
                                    "Copied OpenConsole.exe to {}",
                                    open_console_target.display()
                                ),
                                Err(e) => println!(
                                    "cargo::warning=Failed to copy OpenConsole.exe from {}: {}",
                                    open_console_source.display(),
                                    e
                                ),
                            }
                        }
                        Ok(output) => {
                            println!(
                                "cargo::warning=Failed to extract conpty nupkg: {}",
                                String::from_utf8_lossy(&output.stderr)
                            );
                        }
                        Err(e) => {
                            println!(
                                "cargo::warning=Failed to run PowerShell for extraction: {}",
                                e
                            );
                        }
                    }
                }
                Ok(output) => {
                    println!(
                        "cargo::warning=Failed to download conpty nupkg: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                Err(e) => {
                    println!(
                        "cargo::warning=Failed to run PowerShell for download: {}",
                        e
                    );
                }
            }
        }

        println!("cargo:rerun-if-env-changed=RELEASE_CHANNEL");
        println!("cargo:rerun-if-env-changed=GITHUB_RUN_NUMBER");

        #[cfg(windows)]
        {
            windows_resources::compile(false).expect("failed to compile Windows resources");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    prepare_app_icon_x11();
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn icon_path() -> std::path::PathBuf {
    use std::str::FromStr;

    let release_channel = option_env!("RELEASE_CHANNEL").unwrap_or("dev");
    let channel = match release_channel {
        "stable" => "",
        "preview" => "-preview",
        "nightly" => "-nightly",
        "dev" => "-dev",
        _ => "-dev",
    };

    #[cfg(windows)]
    let icon = format!("resources/windows/app-icon{}.ico", channel);
    #[cfg(not(windows))]
    let icon = format!("resources/app-icon{}.png", channel);

    std::path::PathBuf::from_str(&icon).unwrap()
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn prepare_app_icon_x11() {
    use image::{ImageReader, imageops};
    use std::env;
    use std::path::Path;

    let out_dir = env::var("OUT_DIR").unwrap();

    let resized_image = ImageReader::open(icon_path())
        .unwrap()
        .decode()
        .unwrap()
        .resize(256, 256, imageops::FilterType::Lanczos3);

    // name should match include_bytes! call in src/zed.rs
    let icon_out_path = Path::new(&out_dir).join("app_icon.png");
    resized_image.save(&icon_out_path).expect("saving app icon");

    println!("cargo:rerun-if-env-changed=RELEASE_CHANNEL");
    println!("cargo:rerun-if-changed={}", icon_path().to_string_lossy());
}

/// The fork's own version and the upstream release it is based on.
struct ZednotesPin {
    version: String,
    upstream_zed_version: String,
    upstream_commit: String,
}

/// Emit the fork's version pin as compile-time environment variables.
///
/// Reading the pin at build time rather than at runtime means a released binary
/// keeps reporting the pin it was built from, even once the checkout has moved on.
fn emit_zednotes_pin() {
    let path = std::path::Path::new("../../zednotes.toml");
    println!("cargo:rerun-if-changed={}", path.display());

    if !path.exists() {
        // No pin present: callers fall back to the upstream crate version.
        return;
    }

    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) => {
            println!("cargo::error=failed to read {}: {error}", path.display());
            std::process::exit(1);
        }
    };

    let pin = match parse_zednotes_pin(&contents) {
        Ok(pin) => pin,
        Err(error) => {
            println!("cargo::error=invalid {}: {error}", path.display());
            std::process::exit(1);
        }
    };

    println!("cargo:rustc-env=ZEDNOTES_VERSION={}", pin.version);
    println!(
        "cargo:rustc-env=ZEDNOTES_UPSTREAM_ZED_VERSION={}",
        pin.upstream_zed_version
    );
    println!(
        "cargo:rustc-env=ZEDNOTES_UPSTREAM_COMMIT={}",
        pin.upstream_commit
    );
}

fn parse_zednotes_pin(contents: &str) -> Result<ZednotesPin, String> {
    let table: toml::Table = toml::from_str(contents).map_err(|error| error.to_string())?;

    let (Some(version), Some(upstream_zed_version), Some(upstream_commit)) = (
        lookup(&table, &["zednotes", "version"]),
        lookup(&table, &["upstream", "zed_version"]),
        lookup(&table, &["upstream", "commit"]),
    ) else {
        return Err(
            "expected `zednotes.version`, `upstream.zed_version` and `upstream.commit`".to_string(),
        );
    };

    Ok(ZednotesPin {
        version: version.to_string(),
        upstream_zed_version: upstream_zed_version.to_string(),
        upstream_commit: upstream_commit.to_string(),
    })
}

fn lookup<'a>(table: &'a toml::Table, keys: &[&str]) -> Option<&'a str> {
    let (first, rest) = keys.split_first()?;
    let mut value = table.get(*first)?;
    for key in rest {
        value = value.get(*key)?;
    }
    value.as_str()
}
