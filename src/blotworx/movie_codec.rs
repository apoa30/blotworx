//! Pinned, isolated FFmpeg codec worker. No Python runtime or system installation.
use anyhow::{ensure, Context, Result};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub const ARCHIVE_URL:&str="https://files.pythonhosted.org/packages/2c/c6/fa760e12a2483469e2bf5058c5faff664acf66cadb4df2ad6205b016a73d/imageio_ffmpeg-0.6.0-py3-none-win_amd64.whl";
pub const ARCHIVE_SHA: &str = "02fa47c83703c37df6bfe4896aab339013f62bf02c5ebf2dce6da56af04ffc0a";
pub const EXE_SHA: &str = "2ce797a0f88d7f067180338fb227f7b1928ea727bd9a4d7a1d022f7c52af71a3";
#[cfg(not(target_os = "horizon"))]
pub fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[cfg(target_os = "horizon")]
pub fn digest(_bytes: &[u8]) -> String {
    String::new()
}
pub fn command(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c.stdin(Stdio::null());
    c
}
fn valid(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|b| digest(&b) == EXE_SHA)
}
#[cfg(target_os = "android")]
pub fn provision() -> Result<PathBuf> {
    let directory = std::env::var_os("BLOTWORX_ANDROID_CODECS").context("Android codec directory is unavailable")?;
    let path = PathBuf::from(directory).join("libbwx_ffmpeg.so");
    ensure!(path.is_file(), "This Android build does not include the Bink codec");
    Ok(path)
}

#[cfg(target_os = "horizon")]
pub fn provision() -> Result<PathBuf> {
    anyhow::bail!("External movie codec binary is unsupported on Nintendo 3DS")
}

#[cfg(not(any(target_os = "android", target_os = "horizon")))]
pub fn provision() -> Result<PathBuf> {
    static PROVISION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = PROVISION
        .lock()
        .map_err(|_| anyhow::anyhow!("Movie codec provisioning lock poisoned"))?;
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "Movie codec provisioning is currently available for Windows x64"
    );
    if let Ok(exe) = std::env::current_exe() {
        let bundled = exe
            .parent()
            .context("Executable has no folder")?
            .join("Codecs/ffmpeg.exe");
        if valid(&bundled) {
            return Ok(bundled);
        }
    }
    let cache = dirs::cache_dir()
        .context("No codec cache directory")?
        .join("Blotworx Engine/runtime/ffmpeg-7.1");
    let exe = cache.join("ffmpeg.exe");
    if valid(&exe) {
        return Ok(exe);
    }
    // Other movie requests may provision concurrently in separate local games.
    // Install by a unique temporary filename, after validating both layers.
    std::fs::create_dir_all(&cache)?;
    let bytes = ureq::get(ARCHIVE_URL)
        .call()?
        .body_mut()
        .with_config()
        .limit(31_246_824 + 1)
        .read_to_vec()?;
    ensure!(
        digest(&bytes) == ARCHIVE_SHA,
        "Movie codec archive checksum mismatch"
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut binary = Vec::new();
    zip.by_name("imageio_ffmpeg/binaries/ffmpeg-win-x86_64-v7.1.exe")?
        .read_to_end(&mut binary)?;
    ensure!(
        digest(&binary) == EXE_SHA,
        "Movie codec executable checksum mismatch"
    );
    if valid(&exe) {
        return Ok(exe);
    }
    let temp = cache.join(format!("ffmpeg-{}.tmp", std::process::id()));
    std::fs::write(&temp, &binary)?;
    if exe.exists() {
        std::fs::remove_file(&exe)?;
    }
    std::fs::rename(&temp, &exe)?;
    let mut license = String::new();
    zip.by_name("imageio_ffmpeg-0.6.0.dist-info/LICENSE")?
        .read_to_string(&mut license)?;
    std::fs::write(cache.join("imageio-LICENSE.txt"), license)?;
    let output = command(&exe).arg("-L").output()?;
    ensure!(output.status.success(), "Movie codec license query failed");
    let mut notice = output.stdout;
    notice.extend(output.stderr);
    notice.extend_from_slice(format!("\nPinned distribution: {ARCHIVE_URL}\nArchive SHA256: {ARCHIVE_SHA}\nExecutable SHA256: {EXE_SHA}\nFFmpeg source: https://github.com/FFmpeg/FFmpeg/tree/n7.1\nBuild provider: https://www.gyan.dev/ffmpeg/builds/\n").as_bytes());
    std::fs::write(cache.join("FFmpeg-LICENSE.txt"), notice)?;
    Ok(exe)
}
