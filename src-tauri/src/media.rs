//! FFmpeg / FFprobe helpers. FFmpeg must be on PATH (see README).

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

pub const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "mov", "flv", "ts", "webm"];

/// Puts the app in a Windows job object that ends every process inside it when the app
/// exits. Without it, FFmpeg and whisper keep running after the app closes (even through
/// Quit), and the job resumed on the next launch writes the same file at the same time: that
/// produced a corrupted 21 GB upload copy.
#[cfg(windows)]
pub fn end_helpers_with_app() -> Result<()> {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: plain Win32 calls with a zeroed, correctly sized info struct. The job handle is
    // left open on purpose: Windows closes it when the process ends, which ends the job.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(anyhow!("couldn't create a job object"));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        if SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size) == 0 {
            return Err(anyhow!("couldn't configure the job object"));
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err(anyhow!("couldn't join the job object"));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn end_helpers_with_app() -> Result<()> {
    Ok(())
}

/// A command that never flashes a console window on Windows.
/// 16 hex digits of the SHA-256 of `s`: a short, stable cache key.
pub fn short_hash(s: &str) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(s.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

pub fn hidden(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    c.kill_on_drop(true);
    c
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct ProbeInfo {
    pub duration_s: Option<f64>,
    pub size: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

pub async fn probe(path: &str) -> Result<ProbeInfo> {
    let out = hidden("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams", path])
        .output()
        .await
        .context("could not run ffprobe; is FFmpeg installed and on PATH?")?;
    if !out.status.success() {
        return Err(anyhow!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    parse_probe(&serde_json::from_slice(&out.stdout)?)
}

pub fn parse_probe(v: &Value) -> Result<ProbeInfo> {
    let num = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64());
    let fmt = &v["format"];
    let mut info = ProbeInfo {
        duration_s: num(&fmt["duration"]),
        size: num(&fmt["size"]).map(|f| f as i64),
        ..Default::default()
    };
    let streams = v["streams"].as_array().ok_or_else(|| anyhow!("ffprobe returned no streams"))?;
    for s in streams {
        match s["codec_type"].as_str() {
            Some("video") if info.video_codec.is_none() => {
                info.video_codec = s["codec_name"].as_str().map(String::from);
                info.width = s["width"].as_i64();
                info.height = s["height"].as_i64();
                info.fps = s["avg_frame_rate"].as_str().and_then(parse_rate);
            }
            Some("audio") if info.audio_codec.is_none() => {
                info.audio_codec = s["codec_name"].as_str().map(String::from);
            }
            _ => {}
        }
    }
    if info.video_codec.is_none() {
        return Err(anyhow!("file has no video stream"));
    }
    Ok(info)
}

fn parse_rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0).then(|| (n / d * 100.0).round() / 100.0)
}

/// (free, total) bytes on the drive holding `path`.
#[cfg(windows)]
pub fn disk_space(path: &str) -> Option<(u64, u64)> {
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: valid NUL-terminated UTF-16 path and out-pointers to live u64s.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) };
    (ok != 0).then_some((avail, total))
}

#[cfg(not(windows))]
pub fn disk_space(_path: &str) -> Option<(u64, u64)> {
    None
}

#[derive(Debug, Serialize)]
pub struct Toolchain {
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
    pub nvenc: bool,
    pub gpu: Option<String>,
}

/// Detects FFmpeg, its NVENC support, and the NVIDIA GPU name.
pub async fn detect_toolchain() -> Toolchain {
    async fn first_line(prog: &str, args: &[&str]) -> Option<String> {
        let out = hidden(prog).args(args).output().await.ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_string())
    }
    let encoders = hidden("ffmpeg").args(["-hide_banner", "-encoders"]).output().await.ok();
    Toolchain {
        ffmpeg: first_line("ffmpeg", &["-version"]).await,
        ffprobe: first_line("ffprobe", &["-version"]).await,
        nvenc: encoders.is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("h264_nvenc")),
        gpu: first_line("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"]).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_ffprobe_output() {
        let v = json!({
            "format": {"duration": "7265.432", "size": "4200000000"},
            "streams": [
                {"codec_type": "audio", "codec_name": "aac"},
                {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "avg_frame_rate": "60000/1001"}
            ]
        });
        let p = parse_probe(&v).unwrap();
        assert_eq!(p.duration_s, Some(7265.432));
        assert_eq!(p.width, Some(1920));
        assert_eq!(p.fps, Some(59.94));
        assert_eq!(p.audio_codec.as_deref(), Some("aac"));
    }

    #[test]
    fn rejects_audio_only_files() {
        let v = json!({"format": {}, "streams": [{"codec_type": "audio", "codec_name": "aac"}]});
        assert!(parse_probe(&v).is_err());
    }
}
