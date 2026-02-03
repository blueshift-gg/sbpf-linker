use std::{ffi::OsString, fs, path::PathBuf, process::Command};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use walkdir::WalkDir;

const LLVM_REPO: &str = "https://github.com/blueshift-gg/llvm-project.git";
const LLVM_BRANCH: &str = "upstream-gallery-21";
const GIT_DEPTH: &str = "1";

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Build automation for sbpf-linker")]
struct CommandLine {
    #[command(subcommand)]
    subcommand: XtaskSubcommand,
}

#[derive(Subcommand)]
enum XtaskSubcommand {
    /// Clone, build, and install LLVM from source.
    BuildLlvm(BuildLlvm),
}

#[derive(clap::Args)]
struct BuildLlvm {
    /// Source directory.
    #[arg(long, default_value = "./llvm-project")]
    src_dir: PathBuf,
    /// Build directory.
    #[arg(long, default_value = "./llvm-build")]
    build_dir: PathBuf,
    /// Directory in which the built LLVM artifacts are installed.
    #[arg(long, default_value = "./llvm-install")]
    install_prefix: PathBuf,
}

fn main() -> Result<()> {
    let CommandLine { subcommand } = CommandLine::parse();
    match subcommand {
        XtaskSubcommand::BuildLlvm(options) => build_llvm(options),
    }
}

fn build_llvm(options: BuildLlvm) -> Result<()> {
    let BuildLlvm { src_dir, build_dir, install_prefix } = options;

    // Remove existing LLVM source if it exists.
    // if we don't remove it, Cmake complains about missing files
    if src_dir.exists() {
        println!("Removing existing LLVM source at {}", src_dir.display());
        fs::remove_dir_all(&src_dir).with_context(|| {
            format!("failed to remove {}", src_dir.display())
        })?;
    }

    println!("Cloning LLVM fork into {}", src_dir.display());
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
            .arg(&src_dir),
        "clone llvm-project",
    )?;

    if !build_dir.exists() {
        fs::create_dir_all(&build_dir).with_context(|| {
            format!("failed to create build dir {}", build_dir.display())
        })?;
    }
    if !install_prefix.exists() {
        fs::create_dir_all(&install_prefix).with_context(|| {
            format!(
                "failed to create install prefix {}",
                install_prefix.display()
            )
        })?;
    }

    // Build flags tuned for the upstream gallery fork.
    let mut install_arg = OsString::from("-DCMAKE_INSTALL_PREFIX=");
    install_arg.push(install_prefix.as_os_str());
    let mut cmake_configure = Command::new("cmake");
    let cmake_configure = cmake_configure
        .arg("-S")
        .arg(src_dir.join("llvm"))
        .arg("-B")
        .arg(&build_dir)
        .args([
            "-G",
            "Ninja",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DLLVM_ENABLE_PROJECTS=",
            "-DLLVM_ENABLE_RUNTIMES=",
            "-DLLVM_TARGETS_TO_BUILD=BPF",
            "-DLLVM_BUILD_TESTS=ON",
            "-DLLVM_INCLUDE_TESTS=ON",
            "-DLLVM_ENABLE_ASSERTIONS=ON",
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
        .arg(build_dir)
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
    for entry in WalkDir::new(&install_prefix).follow_links(false) {
        let entry = entry.with_context(|| {
            format!(
                "failed to read filesystem entry while traversing install prefix {}",
                install_prefix.display()
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
    let llvm_config = install_prefix.join("bin").join("llvm-config");
    if llvm_config.exists() {
        let output = Command::new(&llvm_config)
            .arg("--version")
            .output()
            .with_context(|| {
                format!("failed to run {} --version", llvm_config.display())
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
