use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use std::env;
use std::fs;
use std::fs::create_dir;
use std::io::Write;
use std::path::{Path, PathBuf};
use xshell::{Shell, cmd};

const QEMU_COMMAND: &str = "qemu-system-x86_64";
const QEMU_COMMON_ARGS: &[&str] = &[
    "-M",
    "q35",
    "-cpu",
    "max",
    "-serial",
    "stdio",
    "-smp",
    "4",
    "-m",
    "2G",
    "-display",
    "sdl",
    "--enable-kvm",
];
const GIT_COMMAND: &str = "git";
const MAKE_COMMAND: &str = "make";
const XORRISO_COMMAND: &str = "xorriso";
const KERNEL_PKG: &str = "aether-kernel";
const ISO_ROOT_DIR: &str = "target/iso-root";
const LIMINE_DIR: &str = "target/limine";
const ISO_KERNEL_PATH: &str = "boot/aether-kernel";
const ISO_CONFIG_PATH: &str = "limine.conf";
const ISO_IMAGE_NAME: &str = "aether-kernel.iso";
const USER_DIR: &str = "user";
const INITRAMFS_IMAGE_NAME: &str = "initramfs.img";
const INITRAMFS_BUILD_SCRIPT: &str = "user/build_initramfs.sh";

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    fn rust_target(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-unknown-none",
            Self::Aarch64 => "aarch64-unknown-none",
        }
    }

    fn apk_arch(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    fn initramfs_dir(self) -> &'static str {
        match self {
            Self::X86_64 => "initramfs-x86_64",
            Self::Aarch64 => "initramfs-aarch64",
        }
    }
}

#[derive(Parser)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value = "x86-64")]
    arch: Arch,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build,
    Initramfs,
    Iso,
    Run,
    RunIso,
    Test,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let sh = Shell::new()?;
    sh.change_dir(project_root());
    let arch = cli.arch;

    match cli.command {
        Commands::Build => build(&sh, arch)?,
        Commands::Initramfs => build_initramfs(&sh, arch)?,
        Commands::Iso => build_iso(&sh, arch)?,
        Commands::Run => run(&sh, arch)?,
        Commands::RunIso => run_iso(&sh, arch)?,
        Commands::Test => test(&sh)?,
    }
    Ok(())
}

fn build(sh: &Shell, arch: Arch) -> Result<()> {
    let target = arch.rust_target();
    println!("> Building kernel for {}", target);
    cmd!(
        sh,
        "cargo build --release --target {target} -p {KERNEL_PKG}"
    )
    .run()
    .context("Build failed")?;
    Ok(())
}

fn run(sh: &Shell, arch: Arch) -> Result<()> {
    run_iso(sh, arch)
}

fn build_iso(sh: &Shell, arch: Arch) -> Result<()> {
    ensure_x86_64(arch, "iso")?;
    build(sh, arch)?;
    let initramfs = build_initramfs_if_present(sh, arch)?;

    let limine_dir = ensure_limine(sh)?;
    let iso_root = iso_root_path();
    let iso_boot_dir = iso_root.join("boot");
    let iso_efi_dir = iso_root.join("EFI").join("BOOT");
    let iso_image = iso_image_path();

    if iso_root.exists() {
        fs::remove_dir_all(&iso_root).context("Failed to clean previous ISO root")?;
    }

    fs::create_dir_all(&iso_boot_dir).context("Failed to create ISO boot directory")?;
    fs::create_dir_all(&iso_efi_dir).context("Failed to create ISO EFI directory")?;

    fs::copy(kernel_elf_path(), iso_root.join(ISO_KERNEL_PATH))
        .context("Failed to copy kernel ELF into ISO root")?;
    fs::write(
        iso_root.join(ISO_CONFIG_PATH),
        limine_cfg(initramfs.is_some()),
    )
    .context("Failed to write limine.conf")?;
    if let Some(initramfs) = initramfs {
        fs::copy(initramfs, iso_root.join("boot").join(INITRAMFS_IMAGE_NAME))
            .context("Failed to copy initramfs image into ISO root")?;
    }

    copy_required_file(
        &limine_dir.join("limine-bios.sys"),
        &iso_boot_dir.join("limine-bios.sys"),
    )?;
    copy_required_file(
        &limine_dir.join("limine-bios-cd.bin"),
        &iso_boot_dir.join("limine-bios-cd.bin"),
    )?;
    copy_required_file(
        &limine_dir.join("limine-uefi-cd.bin"),
        &iso_boot_dir.join("limine-uefi-cd.bin"),
    )?;
    copy_required_file(
        &limine_dir.join("BOOTX64.EFI"),
        &iso_efi_dir.join("BOOTX64.EFI"),
    )?;

    if limine_dir.join("BOOTIA32.EFI").exists() {
        fs::copy(
            limine_dir.join("BOOTIA32.EFI"),
            iso_efi_dir.join("BOOTIA32.EFI"),
        )
        .context("Failed to copy BOOTIA32.EFI")?;
    }

    println!("> Building Limine ISO {}", iso_image.display());
    cmd!(
        sh,
        "{XORRISO_COMMAND} -as mkisofs -b boot/limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table --efi-boot boot/limine-uefi-cd.bin -efi-boot-part --efi-boot-image --protective-msdos-label {iso_root} -o {iso_image}"
    )
    .run()
    .context("ISO build failed")?;

    let limine_exe = limine_executable(&limine_dir)?;
    println!("> Installing Limine BIOS boot sector");
    cmd!(sh, "{limine_exe} bios-install {iso_image}")
        .run()
        .context("limine bios-install failed")?;

    Ok(())
}

fn build_initramfs(sh: &Shell, arch: Arch) -> Result<()> {
    let Some(image) = build_initramfs_if_present(sh, arch)? else {
        return Err(anyhow!(
            "No `{}` directory found at project root",
            arch.initramfs_dir()
        ));
    };

    println!("> Built initramfs {}", image.display());
    Ok(())
}

fn run_iso(sh: &Shell, arch: Arch) -> Result<()> {
    ensure_x86_64(arch, "run-iso")?;
    build_iso(sh, arch)?;
    let iso_image = iso_image_path();

    cmd!(
        sh,
        "{QEMU_COMMAND} {QEMU_COMMON_ARGS...} -cdrom {iso_image}"
    )
    .run()
    .context("QEMU failed")?;
    Ok(())
}

fn test(sh: &Shell) -> Result<()> {
    cmd!(sh, "cargo test --workspace").run()?;
    Ok(())
}

fn ensure_limine(sh: &Shell) -> Result<PathBuf> {
    if let Some(dir) = env::var_os("LIMINE_DIR") {
        let path = PathBuf::from(dir);
        validate_limine_dir(&path)?;
        return Ok(path);
    }

    let limine_dir = project_root().join(LIMINE_DIR);
    if !limine_dir.exists() {
        println!("> Fetching Limine sources");
        cmd!(
            sh,
            "{GIT_COMMAND} clone --depth=1 --branch=v11.x-binary https://github.com/limine-bootloader/limine.git {limine_dir}"
        )
        .run()
        .context("Failed to clone Limine repository")?;
    }

    if !limine_dir.join("limine").exists() {
        println!("> Building Limine host utility and boot assets");
        cmd!(sh, "{MAKE_COMMAND} -C {limine_dir}")
            .run()
            .context("Failed to build Limine")?;
    }

    validate_limine_dir(&limine_dir)?;
    Ok(limine_dir)
}

fn validate_limine_dir(path: &Path) -> Result<()> {
    for required in [
        "limine",
        "limine-bios.sys",
        "limine-bios-cd.bin",
        "limine-uefi-cd.bin",
        "BOOTX64.EFI",
    ] {
        let file = path.join(required);
        if !file.exists() {
            return Err(anyhow!("Missing Limine asset: {}", file.display()));
        }
    }

    Ok(())
}

fn limine_executable(path: &Path) -> Result<PathBuf> {
    let exe = path.join("limine");
    if exe.exists() {
        Ok(exe)
    } else {
        Err(anyhow!("Missing Limine host utility: {}", exe.display()))
    }
}

fn copy_required_file(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst).with_context(|| format!("Failed to copy {}", src.display()))?;
    Ok(())
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn kernel_elf_path() -> PathBuf {
    project_root()
        .join("target")
        .join(Arch::X86_64.rust_target())
        .join("release")
        .join(KERNEL_PKG)
}

fn iso_root_path() -> PathBuf {
    project_root().join(ISO_ROOT_DIR)
}

fn iso_image_path() -> PathBuf {
    project_root().join("target").join(ISO_IMAGE_NAME)
}

fn initramfs_dir(arch: Arch) -> PathBuf {
    project_root().join(arch.initramfs_dir())
}

fn initramfs_image_path(arch: Arch) -> PathBuf {
    match arch {
        Arch::X86_64 => project_root().join("target").join(INITRAMFS_IMAGE_NAME),
        _ => project_root()
            .join("target")
            .join(arch.apk_arch())
            .join(INITRAMFS_IMAGE_NAME),
    }
}

fn build_initramfs_if_present(sh: &Shell, arch: Arch) -> Result<Option<PathBuf>> {
    let root = initramfs_dir(arch);
    if !root.exists() {
        if create_dir(root.clone()).is_err() {
            return Ok(None);
        }
    }

    populate_initramfs_root(sh, arch, &root)?;

    let image = initramfs_image_path(arch);
    if let Some(parent) = image.parent() {
        fs::create_dir_all(parent).context("Failed to create initramfs output directory")?;
    }

    let mut archive = Vec::new();
    write_newc_tree(&root, &root, &mut archive)?;
    write_trailer(&mut archive)?;
    fs::write(&image, archive).context("Failed to write initramfs image")?;
    Ok(Some(image))
}

fn populate_initramfs_root(sh: &Shell, arch: Arch, root: &Path) -> Result<()> {
    let script = project_root().join(INITRAMFS_BUILD_SCRIPT);
    if !script.exists() {
        return Err(anyhow!(
            "Missing initramfs build script: {}",
            script.display()
        ));
    }

    let user_dir = project_root().join(USER_DIR);
    fs::create_dir_all(root).with_context(|| format!("Failed to create {}", root.display()))?;
    let _arch = sh.push_env("ARCH", arch.apk_arch());
    let _sysroot = sh.push_env("SYSROOT", root);
    let _user = sh.push_env("USER_DIR", user_dir);
    cmd!(sh, "bash {script}")
        .run()
        .context("initramfs build script failed")
}

fn write_newc_tree(root: &Path, path: &Path, out: &mut Vec<u8>) -> Result<()> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("Failed to stat {}", path.display()))?;

    if path != root {
        let relative = path
            .strip_prefix(root)
            .context("Failed to compute relative initramfs path")?;
        let name = relative.to_string_lossy().replace('\\', "/");
        if metadata.file_type().is_dir() {
            write_newc_entry(out, &name, 0o040755, &[])?;
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(path)
                .with_context(|| format!("Failed to read symlink {}", path.display()))?;
            let target = target.to_string_lossy();
            write_newc_entry(out, &name, 0o120777, target.as_bytes())?;
        } else if metadata.file_type().is_file() {
            let bytes =
                fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
            let mode = if is_executable(path) {
                0o100755
            } else {
                0o100644
            };
            write_newc_entry(out, &name, mode, &bytes)?;
        }
    }

    if metadata.file_type().is_dir() {
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("Failed to enumerate {}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("Failed to enumerate {}", path.display()))?;
        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            write_newc_tree(root, &entry.path(), out)?;
        }
    }

    Ok(())
}

fn write_newc_entry(out: &mut Vec<u8>, name: &str, mode: u32, data: &[u8]) -> Result<()> {
    let namesize = name.len() + 1;
    write!(
        out,
        "070701{ino:08x}{mode:08x}{uid:08x}{gid:08x}{nlink:08x}{mtime:08x}{filesize:08x}{devmajor:08x}{devminor:08x}{rdevmajor:08x}{rdevminor:08x}{namesize:08x}{check:08x}",
        ino = 0,
        mode = mode,
        uid = 0,
        gid = 0,
        nlink = 1,
        mtime = 0,
        filesize = data.len(),
        devmajor = 0,
        devminor = 0,
        rdevmajor = 0,
        rdevminor = 0,
        namesize = namesize,
        check = 0
    )
    .context("Failed to serialize initramfs header")?;
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    align_newc(out);
    out.extend_from_slice(data);
    align_newc(out);
    Ok(())
}

fn write_trailer(out: &mut Vec<u8>) -> Result<()> {
    write_newc_entry(out, "TRAILER!!!", 0, &[])
}

fn align_newc(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn is_executable(path: &Path) -> bool {
    path.extension().is_none() || path.file_name().is_some_and(|name| name == "init")
}

fn limine_cfg(with_initramfs: bool) -> String {
    let mut config = String::from(
        r#"timeout: 0

/Aether Kernel
    protocol: limine
    path: boot():/boot/aether-kernel

    kernel_cmdline: init=/usr/lib/aether/init
"#,
    );
    if with_initramfs {
        config.push_str("\n    module_path: boot():/boot/initramfs.img\n");
    }
    config
}

fn ensure_x86_64(arch: Arch, command: &str) -> Result<()> {
    match arch {
        Arch::X86_64 => Ok(()),
        _ => Err(anyhow!(
            "`xtask {command}` currently only supports x86_64 boot media/QEMU"
        )),
    }
}
