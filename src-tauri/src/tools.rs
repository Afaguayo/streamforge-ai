//! Downloads and locates external engines: whisper.cpp (speech-to-text), its models, and the
//! optional sherpa-onnx speaker separator. Versions are pinned so an upstream release can't
//! break installs.

use crate::db::Db;
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const WHISPER_TAG: &str = "b5454";
const WHISPER_CUDA_ZIP: &str = "whisper-bin-win-cuda-12.4.0-x64.zip";
const WHISPER_CPU_ZIP: &str = "whisper-bin-x64.zip";

const VAD_MODEL: &str = "ggml-silero-v5.1.2.bin";
const SHERPA_TAG: &str = "v2.0.1";
const SHERPA_DIAR_EXE: &str = "sherpa-onnx-offline-speaker-diarization.exe";

pub fn whisper_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools").join("whisper")
}

pub fn whisper_exe(data_dir: &Path) -> PathBuf {
    whisper_dir(data_dir).join("Release").join("whisper-cli.exe")
}

pub fn model_path(data_dir: &Path, model: &str) -> PathBuf {
    whisper_dir(data_dir).join("models").join(format!("ggml-{model}.bin"))
}

pub fn vad_path(data_dir: &Path) -> PathBuf {
    whisper_dir(data_dir).join("models").join(VAD_MODEL)
}

fn speakers_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools").join("speakers")
}

pub struct SpeakerTool {
    pub exe: PathBuf,
    pub segmentation: PathBuf,
    pub embedding: PathBuf,
}

/// The speaker separator, once it's been installed from Settings.
pub fn speaker_tool(data_dir: &Path) -> Option<SpeakerTool> {
    let d = speakers_dir(data_dir);
    let t = SpeakerTool { exe: d.join("bin").join(SHERPA_DIAR_EXE), segmentation: d.join("segmentation").join("model.onnx"), embedding: d.join("embedding.onnx") };
    (t.exe.is_file() && t.segmentation.is_file() && t.embedding.is_file()).then_some(t)
}

#[derive(Serialize)]
pub struct EngineStatus {
    pub whisper_installed: bool,
    pub model_installed: bool,
    pub model: String,
    pub speakers_installed: bool,
}

pub fn status(data_dir: &Path, model: &str) -> EngineStatus {
    EngineStatus {
        whisper_installed: whisper_exe(data_dir).is_file(),
        model_installed: model_path(data_dir, model).is_file(),
        model: model.to_string(),
        speakers_installed: speaker_tool(data_dir).is_some(),
    }
}

/// Streams `url` to `dest` (via a .part file), reporting progress 0..1 to `on_progress`.
async fn download(url: &str, dest: &Path, mut on_progress: impl FnMut(f64, u64)) -> Result<()> {
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).build()?;
    let mut resp = client.get(url).send().await.with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("download failed ({}) for {url}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let part = dest.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut got = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk)?;
        got += chunk.len() as u64;
        if total > 0 {
            on_progress(got as f64 / total as f64, got);
        }
    }
    file.flush()?;
    drop(file);
    if total > 0 && got != total {
        return Err(anyhow!("download was cut off ({got} of {total} bytes); it will retry"));
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

/// A plain download with no progress report.
pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    download(url, dest, |_, _| {}).await
}

fn unzip(zip_path: &Path, into: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else { continue }; // skips unsafe "../" paths
        let out = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            std::fs::create_dir_all(out.parent().unwrap())?;
            std::io::copy(&mut entry, &mut std::fs::File::create(&out)?)?;
        }
    }
    Ok(())
}

/// Voice detection model: a small file, fetched the first time it's needed.
pub async fn ensure_vad(data_dir: &Path) -> Result<PathBuf> {
    let p = vad_path(data_dir);
    if !p.is_file() {
        download(&format!("https://huggingface.co/ggml-org/whisper-vad/resolve/main/{VAD_MODEL}"), &p, |_, _| {}).await?;
    }
    Ok(p)
}

async fn untar(archive: &Path, into: &Path) -> Result<()> {
    std::fs::create_dir_all(into)?;
    let out = crate::media::hidden("tar")
        .arg("-xjf").arg(archive)
        .arg("-C").arg(into)
        .arg("--strip-components=1")
        .output()
        .await
        .context("couldn't run tar")?;
    if !out.status.success() {
        return Err(anyhow!("unpacking failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Installs the speaker separator. Only runs when asked for from Settings.
pub async fn install_speakers(db: &Db, job_id: i64, data_dir: &Path) -> Result<String> {
    let dir = speakers_dir(data_dir);
    let base = "https://github.com/k2-fsa/sherpa-onnx/releases/download";
    let asset = format!("sherpa-onnx-{SHERPA_TAG}-win-x64-shared-MD-MinSizeRel-no-tts.tar.bz2");
    let tar = dir.join("engine.tar.bz2");
    download(&format!("{base}/{SHERPA_TAG}/{asset}"), &tar, |p, _| db.job_progress(job_id, p * 0.4, "Downloading speaker engine")).await?;
    untar(&tar, &dir).await?;
    let _ = std::fs::remove_file(&tar);
    let seg = dir.join("segmentation.tar.bz2");
    download(&format!("{base}/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2"), &seg, |p, _| db.job_progress(job_id, 0.4 + p * 0.1, "Downloading speaker models")).await?;
    untar(&seg, &dir.join("segmentation")).await?;
    let _ = std::fs::remove_file(&seg);
    download(&format!("{base}/speaker-recongition-models/nemo_en_titanet_small.onnx"), &dir.join("embedding.onnx"), |p, _| db.job_progress(job_id, 0.5 + p * 0.5, "Downloading speaker models")).await?;
    if speaker_tool(data_dir).is_none() {
        return Err(anyhow!("speaker engine unpacked but files are missing"));
    }
    Ok("Speaker detection ready".into())
}

/// Installs whatever is missing. Progress: engine 0–60 %, model 60–100 %.
pub async fn install_whisper(db: &Db, job_id: i64, data_dir: &Path, model: &str, gpu: bool) -> Result<String> {
    let dir = whisper_dir(data_dir);
    if !whisper_exe(data_dir).is_file() {
        let asset = if gpu { WHISPER_CUDA_ZIP } else { WHISPER_CPU_ZIP };
        let url = format!("https://github.com/ggml-org/whisper.cpp/releases/download/{WHISPER_TAG}/{asset}");
        let zip_path = dir.join(asset);
        download(&url, &zip_path, |p, got| db.job_progress(job_id, p * 0.55, &format!("Downloading speech engine ({} MB)", got / 1_048_576))).await?;
        db.job_progress(job_id, 0.57, "Unpacking speech engine");
        unzip(&zip_path, &dir)?;
        let _ = std::fs::remove_file(&zip_path);
        if !whisper_exe(data_dir).is_file() {
            return Err(anyhow!("speech engine unpacked but whisper-cli.exe is missing"));
        }
    }
    let mp = model_path(data_dir, model);
    if !mp.is_file() {
        let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin");
        download(&url, &mp, |p, got| db.job_progress(job_id, 0.6 + p * 0.4, &format!("Downloading {model} model ({} MB)", got / 1_048_576))).await?;
    }
    Ok(format!("Transcription engine ready ({model} model)"))
}
