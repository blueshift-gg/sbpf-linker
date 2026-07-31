use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

const LLVM_REPO: &str = "https://github.com/llvm/llvm-project.git";
const LLVM_BRANCH: &str = "main";
const LLVM_CACHE_KEY: &str = "llvm-main";
const GIT_DEPTH: &str = "1";

enum CommandName {
    Install,
    UpdateLlvm,
}

fn main() -> Result<()> {
    let command = match std::env::args().nth(1).as_deref() {
        None | Some("install") => CommandName::Install,
        Some("update-llvm") => CommandName::UpdateLlvm,
        Some(argument) => anyhow::bail!(
            "unexpected argument `{argument}`; expected one of: install, update-llvm"
        ),
    };

    match command {
        CommandName::Install => install(),
        CommandName::UpdateLlvm => update_llvm(),
    }
}

fn project_root() -> Result<PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap());

    // If we're in xtask dir, go up one level
    if manifest_dir.ends_with("xtask") {
        Ok(manifest_dir.parent().unwrap().to_path_buf())
    } else {
        Ok(manifest_dir)
    }
}

fn cache_dir() -> PathBuf {
    // Build tools outside the project to avoid Cargo workspace issues
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(format!("sbpf-linker-{LLVM_CACHE_KEY}"))
}

fn clone_llvm_checkout(llvm_src_dir: &Path) -> Result<()> {
    println!("============================================");
    println!(
        "[1/2] Cloning {} {} into {}",
        LLVM_REPO,
        LLVM_BRANCH,
        llvm_src_dir.display()
    );
    println!("============================================");
    run_command(
        Command::new("git")
            .args([
                "clone",
                "--depth",
                GIT_DEPTH,
                "--branch",
                LLVM_BRANCH,
                LLVM_REPO,
            ])
            .arg(llvm_src_dir),
        "clone llvm-project",
    )
}

fn ensure_llvm_checkout(llvm_src_dir: &Path) -> Result<bool> {
    if !llvm_src_dir.exists() {
        println!(
            "LLVM checkout not found at {}; cloning it",
            llvm_src_dir.display()
        );
        clone_llvm_checkout(llvm_src_dir)?;
        return Ok(true);
    }

    println!(
        "Using existing LLVM checkout at {}; skipping update",
        llvm_src_dir.display()
    );
    Ok(false)
}

fn update_llvm_checkout(llvm_src_dir: &Path) -> Result<bool> {
    if !llvm_src_dir.exists() {
        clone_llvm_checkout(llvm_src_dir)?;
        return Ok(true);
    }

    let previous_head = command_stdout(
        Command::new("git")
            .args(["-C"])
            .arg(llvm_src_dir)
            .args(["rev-parse", "HEAD"]),
        "read llvm-project HEAD",
    )?;

    println!(
        "Updating {} from {} {}",
        llvm_src_dir.display(),
        LLVM_REPO,
        LLVM_BRANCH
    );
    run_command(
        Command::new("git").args(["-C"]).arg(llvm_src_dir).args([
            "pull",
            "--ff-only",
            "origin",
            LLVM_BRANCH,
        ]),
        "update llvm-project",
    )?;

    let current_head = command_stdout(
        Command::new("git")
            .args(["-C"])
            .arg(llvm_src_dir)
            .args(["rev-parse", "HEAD"]),
        "read updated llvm-project HEAD",
    )?;

    Ok(previous_head != current_head)
}

struct LlvmPaths {
    src_dir: PathBuf,
    build_dir: PathBuf,
    install_dir: PathBuf,
    config: PathBuf,
}

fn llvm_paths() -> Result<LlvmPaths> {
    let base_dir = cache_dir();
    std::fs::create_dir_all(&base_dir)?;
    let src_dir = base_dir.join("llvm-project");
    let build_dir = base_dir.join("llvm-build");
    let install_dir = base_dir.join("llvm-install");
    let config = install_dir.join("bin/llvm-config");

    Ok(LlvmPaths { src_dir, build_dir, install_dir, config })
}

fn build_llvm_if_needed(
    paths: &LlvmPaths,
    checkout_changed: bool,
) -> Result<()> {
    if !paths.config.exists() || checkout_changed {
        if !paths.build_dir.exists() {
            fs::create_dir_all(&paths.build_dir).with_context(|| {
                format!(
                    "failed to create build dir {}",
                    paths.build_dir.display()
                )
            })?;
        }
        if !paths.install_dir.exists() {
            fs::create_dir_all(&paths.install_dir).with_context(|| {
                format!(
                    "failed to create install prefix {}",
                    paths.install_dir.display()
                )
            })?;
        }

        // Build only the LLVM components needed by sbpf-linker.
        let mut install_arg = OsString::from("-DCMAKE_INSTALL_PREFIX=");
        install_arg.push(paths.install_dir.as_os_str());
        let mut cmake_configure = Command::new("cmake");
        let cmake_configure = cmake_configure
            .arg("-S")
            .arg(paths.src_dir.join("llvm"))
            .arg("-B")
            .arg(&paths.build_dir)
            .args([
                "-G",
                "Ninja",
                "-DCMAKE_BUILD_TYPE=Release",
                "-DLLVM_ENABLE_PROJECTS=",
                "-DLLVM_ENABLE_RUNTIMES=",
                "-DLLVM_TARGETS_TO_BUILD=BPF",
                "-DLLVM_BUILD_LLVM_DYLIB=OFF",
                "-DLLVM_BUILD_TESTS=ON",
                "-DLLVM_INCLUDE_TESTS=ON",
                "-DLLVM_ENABLE_ASSERTIONS=ON",
                "-DLLVM_LINK_LLVM_DYLIB=OFF",
                "-DLLVM_ENABLE_ZLIB=OFF",
                "-DLLVM_ENABLE_ZSTD=OFF",
                "-DLLVM_INSTALL_UTILS=ON",
            ])
            .arg(install_arg);
        println!("Configuring LLVM with command {cmake_configure:?}");
        let status = cmake_configure.status().with_context(|| {
            format!(
                "failed to configure LLVM build with command {cmake_configure:?}"
            )
        })?;
        if !status.success() {
            anyhow::bail!(
                "failed to configure LLVM build with command {cmake_configure:?}: {status}"
            );
        }

        let mut cmake_build = Command::new("cmake");
        let cmake_build = cmake_build
            .arg("--build")
            .arg(&paths.build_dir)
            .args(["--target", "install"])
            // Create symlinks rather than copies to conserve disk space,
            // especially on GitHub-hosted runners.
            //
            // Since the LLVM build creates a bunch of symlinks (and this setting
            // does not turn those into symlinks-to-symlinks), use absolute
            // symlinks so we can distinguish the two cases.
            .env("CMAKE_INSTALL_MODE", "ABS_SYMLINK");
        println!("Building LLVM with command {cmake_build:?}");
        let status = cmake_build.status().with_context(|| {
            format!("failed to build LLVM with command {cmake_configure:?}")
        })?;
        if !status.success() {
            anyhow::bail!(
                "failed to build LLVM with command {cmake_configure:?}: {status}"
            );
        }

        // Confirmation log to show which llvm-config was used.
        // This is just a cosmetic to make sure it worked.
        if paths.config.exists() {
            let output = Command::new(&paths.config)
                .arg("--version")
                .output()
                .with_context(|| {
                    format!(
                        "failed to run {} --version",
                        paths.config.display()
                    )
                })?;
            let version = String::from_utf8_lossy(&output.stdout);
            println!(
                "LLVM config: {} ({})",
                paths.config.display(),
                version.trim()
            );
        } else {
            println!("LLVM config not found at {}", paths.config.display());
        }
    } else {
        println!(
            "LLVM already cloned and built (found {}), skipping",
            paths.config.display()
        );
    }

    Ok(())
}

fn install() -> Result<()> {
    ensure_build_dependencies()?;
    let paths = llvm_paths()?;
    let checkout_changed = ensure_llvm_checkout(&paths.src_dir)?;
    build_llvm_if_needed(&paths, checkout_changed)?;

    println!("============================================");
    println!("[2/2] Building the linker");
    println!("============================================");
    build_linker(&paths.install_dir)
}

fn update_llvm() -> Result<()> {
    ensure_build_dependencies()?;
    let paths = llvm_paths()?;
    let checkout_changed = update_llvm_checkout(&paths.src_dir)?;
    build_llvm_if_needed(&paths, checkout_changed)
}

fn build_linker(llvm_install_dir: &Path) -> Result<()> {
    let project_root = project_root()?;

    let mut cmd = Command::new("cargo");

    if cfg!(target_os = "macos") {
        // Ensure brew prefixes
        let llvm_output = Command::new("brew")
            .args(["--prefix", "llvm"])
            .output()
            .with_context(|| "failed to run brew --prefix llvm")?;
        if !llvm_output.status.success() {
            anyhow::bail!(
                "brew --prefix llvm failed: {}",
                String::from_utf8_lossy(&llvm_output.stderr).trim()
            );
        }
        let llvm_prefix =
            String::from_utf8_lossy(&llvm_output.stdout).trim().to_string();

        let zlib_output = Command::new("brew")
            .args(["--prefix", "zlib"])
            .output()
            .with_context(|| "failed to run brew --prefix zlib")?;
        if !zlib_output.status.success() {
            anyhow::bail!(
                "brew --prefix zlib failed: {}",
                String::from_utf8_lossy(&zlib_output.stderr).trim()
            );
        }
        let zlib_prefix =
            String::from_utf8_lossy(&zlib_output.stdout).trim().to_string();

        let zstd_output = Command::new("brew")
            .args(["--prefix", "zstd"])
            .output()
            .with_context(|| "failed to run brew --prefix zstd")?;
        if !zstd_output.status.success() {
            anyhow::bail!(
                "brew --prefix zstd failed: {}",
                String::from_utf8_lossy(&zstd_output.stderr).trim()
            );
        }
        let zstd_prefix =
            String::from_utf8_lossy(&zstd_output.stdout).trim().to_string();

        if llvm_prefix.is_empty()
            || zlib_prefix.is_empty()
            || zstd_prefix.is_empty()
        {
            anyhow::bail!(
                "failed to resolve brew prefixes (llvm='{}', zlib='{}', zstd='{}')",
                llvm_prefix,
                zlib_prefix,
                zstd_prefix
            );
        }

        cmd.env("CXXSTDLIB_PATH", format!("{}/lib/c++", llvm_prefix));
        cmd.env("ZLIB_PATH", format!("{}/lib", zlib_prefix));
        cmd.env("LIBZSTD_PATH", format!("{}/lib", zstd_prefix));
    }

    cmd.args([
        "install",
        "--path",
        ".",
        "--no-default-features",
        "--features",
        "bpf-linker/llvm-22,bpf-linker/llvm-link-static",
        "--force",
    ])
    .env("LLVM_PREFIX", llvm_install_dir)
    .current_dir(&project_root);

    run_command(&mut cmd, "build sbpf-linker (static)")?;
    Ok(())
}

fn command_stdout(cmd: &mut Command, description: &str) -> Result<String> {
    let output = cmd
        .output()
        .with_context(|| format!("failed to run: {description}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "command failed: {description}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn run_command(cmd: &mut Command, description: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("failed to run: {}", description))?;

    if !status.success() {
        anyhow::bail!("command failed: {}", description);
    }

    Ok(())
}

// On macOS, use Homebrew for the build tools and native libraries.
fn ensure_brew_dependencies() -> Result<()> {
    if !command_available("brew") {
        anyhow::bail!(
            "Homebrew is required to install sbpf-linker on macOS\nhelp: install Homebrew from https://brew.sh and retry"
        );
    }

    let formulas = ["cmake", "ninja", "llvm", "zlib", "zstd"];
    let missing = formulas
        .into_iter()
        .filter(|formula| {
            Command::new("brew")
                .args(["list", "--versions", formula])
                .output()
                .map_or(true, |output| {
                    !output.status.success() || output.stdout.is_empty()
                })
        })
        .collect::<Vec<_>>();

    if !missing.is_empty() {
        println!(
            "Installing missing Homebrew dependencies: {}",
            missing.join(", ")
        );
        let mut command = Command::new("brew");
        command.arg("install").args(missing);
        run_command(&mut command, "install Homebrew dependencies")?;
    }
    Ok(())
}

fn command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn ensure_build_dependencies() -> Result<()> {
    if cfg!(target_os = "macos") {
        ensure_brew_dependencies()?;
    }

    let missing = ["git", "cmake", "ninja"]
        .into_iter()
        .filter(|command| !command_available(command))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!(
            "missing required build tools: {}\nhelp: install them with your system package manager and retry",
            missing.join(", ")
        );
    }
    Ok(())
}
