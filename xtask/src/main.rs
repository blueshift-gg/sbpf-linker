use std::{ffi::OsString, fs, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use walkdir::WalkDir;

const LLVM_REPO: &str = "https://github.com/blueshift-gg/llvm-project.git";
const LLVM_BRANCH: &str = "upstream-gallery-21";
const GIT_DEPTH: &str = "1";

fn main() -> Result<()> {
    build()
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
        .join("sbpf-linker-upstream-gallery")
}

fn build() -> Result<()> {
    let base_dir = cache_dir();
    std::fs::create_dir_all(&base_dir)?;
    let llvm_src_dir = base_dir.join("llvm-project");
    let llvm_build_dir = base_dir.join("llvm-build");
    let llvm_install_dir = base_dir.join("llvm-install");
    let llvm_config = llvm_install_dir.join("bin/llvm-config");

    if !llvm_config.exists() {
        if llvm_src_dir.exists() {
            println!(
                "llvm-project directory already exists ({}), skipping clone",
                llvm_src_dir.display()
            );
        } else {
            println!("============================================");
            println!(
                "[1/2] Cloning LLVM fork into {}",
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
                    .arg(&llvm_src_dir),
                "clone llvm-project",
            )?;
        }

        if !llvm_build_dir.exists() {
            fs::create_dir_all(&llvm_build_dir).with_context(|| {
                format!(
                    "failed to create build dir {}",
                    llvm_build_dir.display()
                )
            })?;
        }
        if !llvm_install_dir.exists() {
            fs::create_dir_all(&llvm_install_dir).with_context(|| {
                format!(
                    "failed to create install prefix {}",
                    llvm_install_dir.display()
                )
            })?;
        }

        if cfg!(target_os = "macos") {
            ensure_brew_dependencies()?;
        }
        // Build flags tuned for the upstream gallery fork.
        let mut install_arg = OsString::from("-DCMAKE_INSTALL_PREFIX=");
        install_arg.push(llvm_install_dir.as_os_str());
        let mut cmake_configure = Command::new("cmake");
        let cmake_configure = cmake_configure
            .arg("-S")
            .arg(llvm_src_dir.join("llvm"))
            .arg("-B")
            .arg(&llvm_build_dir)
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
            .arg(llvm_build_dir)
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

        // Move targets over the symlinks that point to them.
        //
        // This whole dance would be simpler if CMake supported
        // `CMAKE_INSTALL_MODE=MOVE`.
        for entry in WalkDir::new(&llvm_install_dir).follow_links(false) {
            let entry = entry.with_context(|| {
                format!(
                    "failed to read filesystem entry while traversing install prefix {}",
                    llvm_install_dir.display()
                )
            })?;
            if !entry.file_type().is_symlink() {
                continue;
            }

            let link_path = entry.path();
            let target = fs::read_link(link_path).with_context(|| {
                format!("failed to read the link {}", link_path.display())
            })?;
            if target.is_absolute() {
                fs::rename(&target, link_path).with_context(|| {
                    format!(
                        "failed to move the target file {} to the location of the symlink {}",
                        target.display(),
                        link_path.display()
                    )
                })?;
            }
        }

        // Confirmation log to show which llvm-config was used.
        // This is just a cosmetic to make sure it worked.
        let llvm_config = llvm_install_dir.join("bin").join("llvm-config");
        if llvm_config.exists() {
            let output = Command::new(&llvm_config)
                .arg("--version")
                .output()
                .with_context(|| {
                    format!(
                        "failed to run {} --version",
                        llvm_config.display()
                    )
                })?;
            let version = String::from_utf8_lossy(&output.stdout);
            println!(
                "LLVM config: {} ({})",
                llvm_config.display(),
                version.trim()
            );
        } else {
            println!("LLVM config not found at {}", llvm_config.display());
        }
    } else {
        println!(
            "LLVM already cloned and built (found {}), skipping",
            llvm_config.display()
        );
    }

    println!("============================================");
    println!("[2/2] Building the linker");
    println!("============================================");
    build_linker(&llvm_install_dir)
}

fn build_linker(llvm_install_dir: &PathBuf) -> Result<()> {
    let project_root = project_root()?;

    let mut cmd = Command::new("cargo");
    cmd.args([
        "install",
        "--path",
        ".",
        "--no-default-features",
        "--features",
        "upstream-gallery-21,bpf-linker/llvm-link-static",
    ])
    .env("LLVM_SYS_211_PREFIX", llvm_install_dir)
    .current_dir(&project_root);

    if cfg!(target_os = "macos") {
        ensure_brew_dependencies()?;

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

    run_command(&mut cmd, "build sbpf-linker (static)")?;
    Ok(())
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

// On macOS, use Homebrew's llvm for libc++, zlib, and zstd
// (macOS doesn't provide static libraries, and building them from source is complex)
fn ensure_brew_dependencies() -> Result<()> {
    let llvm_installed = Command::new("brew")
        .args(["--prefix", "llvm"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let zlib_installed = Command::new("brew")
        .args(["--prefix", "zlib"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let zstd_installed = Command::new("brew")
        .args(["--prefix", "zstd"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !llvm_installed || !zlib_installed || !zstd_installed {
        println!("  Installing Homebrew dependencies (llvm, zlib, zstd)...");
        run_command(
            Command::new("brew").args(["install", "llvm", "zlib", "zstd"]),
            "install brew dependencies",
        )?;
    }
    Ok(())
}
