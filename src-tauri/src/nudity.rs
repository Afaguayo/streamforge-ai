//! Nudity censor: finds exposed nudity on screen with the NudeNet detector (a small YOLO
//! model, run locally with ONNX Runtime) and builds FFmpeg filters that pixelate it.
//!
//! Frames are sampled at 2 per second, so a hit stands for the half second around it.
//! Game footage gives single-frame false alarms (skin-coloured clothes, statues), so a
//! region needs hits in at least two sampled frames close together.

use crate::media::hidden;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::AsyncReadExt;

/// The detector ships inside the NudeNet Python wheel on PyPI (the GitHub release asset needs a login).
const WHEEL_URL: &str = "https://files.pythonhosted.org/packages/1c/ee/1aa02d44ba958cc77e16ff1e41a0aac5e721037db7bf62b9c9d124917f87/nudenet-3.4.2-py3-none-any.whl";
const WHEEL_SHA256: &str = "5937dbd84e5d8e5de038f08ffea5a1bb50a08475776bf2b4795914ce0eaf0331";
const SIDE: usize = 320;
const FPS: f64 = 2.0;
/// Classes that get censored: exposed buttocks, female breast, female genitalia, anus, male genitalia.
const CENSORED: [usize; 5] = [2, 3, 4, 6, 14];
const CLASSES: usize = 18;
/// Hits closer than this (seconds) belong to the same region.
const GAP: f64 = 1.0;
const MAX_BOXES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub t: f64,
    pub score: f32,
    /// x, y, w, h as fractions of the frame.
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub start: f64,
    pub end: f64,
    /// x, y, w, h as fractions of the frame.
    pub bbox: [f64; 4],
}

pub fn model_path(data_dir: &Path) -> PathBuf {
    data_dir.join("tools").join("nudenet").join("320n.onnx")
}

/// Fetches the detector the first time it's needed (about 12 MB).
pub async fn ensure_model(data_dir: &Path) -> Result<PathBuf> {
    let p = model_path(data_dir);
    if p.is_file() {
        return Ok(p);
    }
    let dir = p.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let wheel = dir.join("nudenet.whl");
    crate::tools::download_file(WHEEL_URL, &wheel).await?;
    let bytes = std::fs::read(&wheel)?;
    use sha2::Digest;
    let sum: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if sum != WHEEL_SHA256 {
        let _ = std::fs::remove_file(&wheel);
        return Err(anyhow!("the nudity model download was corrupted; it will retry"));
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut entry = zip.by_name("nudenet/320n.onnx").context("model missing from the NudeNet package")?;
    let part = p.with_extension("part");
    std::io::copy(&mut entry, &mut std::fs::File::create(&part)?)?;
    std::fs::rename(&part, &p)?;
    let _ = std::fs::remove_file(&wheel);
    Ok(p)
}

/// Turns one model output ([22, 2100]: cx, cy, w, h in the padded 320 square, then 18 class
/// scores) into hits for the censored classes. `aspect` is width / height of the video.
pub fn decode(out: &[f32], anchors: usize, aspect: f64, t: f64, threshold: f32) -> Vec<Hit> {
    // The frame is scaled to fit 320 and padded on the right or bottom, so its content spans
    // cw x ch pixels from the top-left corner.
    let (cw, ch) = if aspect >= 1.0 { (SIDE as f64, SIDE as f64 / aspect) } else { (SIDE as f64 * aspect, SIDE as f64) };
    let at = |row: usize, i: usize| out[row * anchors + i];
    let mut found: Vec<Hit> = Vec::new();
    for i in 0..anchors {
        let (mut best, mut score) = (0, 0f32);
        for c in 0..CLASSES {
            let s = at(4 + c, i);
            if s > score {
                (best, score) = (c, s);
            }
        }
        if score < threshold || !CENSORED.contains(&best) {
            continue;
        }
        let (cx, cy, w, h) = (at(0, i) as f64, at(1, i) as f64, at(2, i) as f64, at(3, i) as f64);
        let x0 = ((cx - w / 2.0) / cw).clamp(0.0, 1.0);
        let y0 = ((cy - h / 2.0) / ch).clamp(0.0, 1.0);
        let x1 = ((cx + w / 2.0) / cw).clamp(0.0, 1.0);
        let y1 = ((cy + h / 2.0) / ch).clamp(0.0, 1.0);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        found.push(Hit { t, score, bbox: [x0, y0, x1 - x0, y1 - y0] });
    }
    nms(found, 0.45)
}

fn iou(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let ix = (a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0]);
    let iy = (a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1]);
    if ix <= 0.0 || iy <= 0.0 {
        return 0.0;
    }
    let inter = ix * iy;
    inter / (a[2] * a[3] + b[2] * b[3] - inter)
}

fn nms(mut hits: Vec<Hit>, limit: f64) -> Vec<Hit> {
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Hit> = Vec::new();
    for h in hits {
        if kept.iter().all(|k| iou(&k.bbox, &h.bbox) < limit) {
            kept.push(h);
        }
    }
    kept
}

/// Groups hits into regions to censor. Hits less than `GAP` apart are one region; a region
/// needs hits in two or more sampled frames, so one-frame false alarms are dropped. Each
/// region covers the union of its boxes, padded, from a half frame-step before its first
/// hit to one after its last.
pub fn regions(hits: &[Hit]) -> Vec<Region> {
    let mut hits = hits.to_vec();
    hits.sort_by(|a, b| a.t.total_cmp(&b.t));
    let step = 1.0 / FPS;
    let mut out = Vec::new();
    let mut group: Vec<Hit> = Vec::new();
    let flush = |group: &mut Vec<Hit>, out: &mut Vec<Region>| {
        let frames = {
            let mut ts: Vec<f64> = group.iter().map(|h| h.t).collect();
            ts.dedup();
            ts.len()
        };
        if frames >= 2 {
            let x0 = group.iter().map(|h| h.bbox[0]).fold(1.0, f64::min);
            let y0 = group.iter().map(|h| h.bbox[1]).fold(1.0, f64::min);
            let x1 = group.iter().map(|h| h.bbox[0] + h.bbox[2]).fold(0.0, f64::max);
            let y1 = group.iter().map(|h| h.bbox[1] + h.bbox[3]).fold(0.0, f64::max);
            let (px, py) = ((x1 - x0) * 0.15 + 0.02, (y1 - y0) * 0.15 + 0.02);
            let (x0, y0, x1, y1) = ((x0 - px).max(0.0), (y0 - py).max(0.0), (x1 + px).min(1.0), (y1 + py).min(1.0));
            out.push(Region {
                start: (group[0].t - step).max(0.0),
                end: group[group.len() - 1].t + step,
                bbox: [x0, y0, x1 - x0, y1 - y0],
            });
        }
        group.clear();
    };
    for h in hits {
        if group.last().is_some_and(|l| h.t - l.t > GAP) {
            flush(&mut group, &mut out);
        }
        group.push(h);
    }
    flush(&mut group, &mut out);
    out
}

/// Regions on another timeline (a clip's, or the cut full stream's), dropping those outside it.
pub fn shift(regions: &[Region], map: impl Fn(f64) -> Option<f64>, len: f64) -> Vec<Region> {
    regions
        .iter()
        .filter_map(|r| {
            let a = map(r.start).or_else(|| map(r.end).map(|e| (e - (r.end - r.start)).max(0.0)))?;
            let b = map(r.end).unwrap_or(a + (r.end - r.start)).min(len);
            (b > a).then_some(Region { start: a, end: b, bbox: r.bbox })
        })
        .collect()
}

/// The file name of a stream's saved scan: the threshold and the source file's path, size and
/// modified time, so a replaced recording is scanned again.
pub fn cache_name(video: &str, threshold: f64) -> String {
    let meta = std::fs::metadata(video).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let mtime = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("nudity_{threshold:.2}_{}.json", crate::media::short_hash(&format!("{video}|{size}|{mtime}")))
}

/// True when a scan read enough of the `expected` frames to trust (some are lost to seeking
/// and rounding). A scan that stopped early has missed whatever came after.
pub fn enough_frames(read: usize, expected: f64) -> bool {
    expected <= 0.0 || read as f64 >= (expected * 0.9).floor()
}

/// Scans `dur` seconds of `video` from `start` (whole video when `dur` is 0). Hit times are
/// relative to `start`. Fails, rather than reporting a clean picture, when FFmpeg fails or
/// stops early.
pub async fn scan(model: &Path, video: &str, start: f64, dur: f64, aspect: f64, threshold: f32, mut progress: impl FnMut(f64)) -> Result<Vec<Hit>> {
    let model = model.to_path_buf();
    let mut session = tokio::task::spawn_blocking(move || -> Result<ort::session::Session> {
        let env = ort::init().with_name("nudity").build()?;
        let builder = ort::session::Session::builder(&env)?.with_intra_threads(4).map_err(|e| -> ort::Error { e.into() })?;
        Ok({ builder }.commit_from_file(&model)?)
    })
    .await??;
    let mut cmd = hidden("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if start > 0.0 {
        cmd.args(["-ss", &format!("{start:.3}")]);
    }
    cmd.arg("-i").arg(video);
    if dur > 0.0 {
        cmd.args(["-t", &format!("{dur:.3}")]);
    }
    cmd.args(["-an", "-vf", &format!("fps={FPS},scale={SIDE}:{SIDE}:force_original_aspect_ratio=decrease,pad={SIDE}:{SIDE}:0:0:black"), "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"]);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("couldn't run FFmpeg")?;
    let mut stdout = child.stdout.take().unwrap();
    // Drained alongside, so a flood of decode errors can't fill the pipe and stall FFmpeg.
    let mut stderr = child.stderr.take().unwrap();
    let errors = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = stderr.read_to_end(&mut b).await;
        String::from_utf8_lossy(&b).into_owned()
    });
    let frame = SIDE * SIDE * 3;
    let total = if dur > 0.0 { (dur * FPS).ceil() } else { 0.0 };
    let mut hits = Vec::new();
    let mut n = 0usize;
    loop {
        if crate::jobs::is_cancelled() {
            let _ = child.kill().await;
            return Err(crate::jobs::Cancelled.into());
        }
        // A batch of frames per trip to the blocking pool keeps the hand-offs cheap.
        let mut batch: Vec<Vec<u8>> = Vec::new();
        for _ in 0..16 {
            let mut buf = vec![0u8; frame];
            match stdout.read_exact(&mut buf).await {
                Ok(_) => batch.push(buf),
                Err(_) => break,
            }
        }
        if batch.is_empty() {
            break;
        }
        let first = n;
        n += batch.len();
        let (s, found) = tokio::task::spawn_blocking(move || -> Result<(ort::session::Session, Vec<Hit>)> {
            let mut found = Vec::new();
            let mut input = vec![0f32; 3 * SIDE * SIDE];
            for (k, rgb) in batch.iter().enumerate() {
                for (p, px) in rgb.chunks_exact(3).enumerate() {
                    for c in 0..3 {
                        input[c * SIDE * SIDE + p] = px[c] as f32 / 255.0;
                    }
                }
                let tensor = ort::value::Tensor::from_array(([1usize, 3, SIDE, SIDE], input.clone().into_boxed_slice()))?;
                let outputs = session.run(ort::inputs!["images" => tensor])?;
                let (shape, data) = outputs[0].try_extract_tensor::<f32>()?;
                let anchors = *shape.last().unwrap_or(&0) as usize;
                found.extend(decode(data, anchors, aspect, (first + k) as f64 / FPS, threshold));
            }
            Ok((session, found))
        })
        .await??;
        session = s;
        hits.extend(found);
        if total > 0.0 {
            progress((n as f64 / total).min(1.0));
        }
    }
    let status = child.wait().await?;
    let errors = errors.await.unwrap_or_default();
    if !status.success() {
        return Err(anyhow!("the nudity scan's FFmpeg failed: {}", errors.lines().rev().take(3).collect::<Vec<_>>().join(" | ")));
    }
    if !enough_frames(n, total) {
        return Err(anyhow!("the nudity scan read only {n} of {total} frames; the video may be damaged"));
    }
    Ok(hits)
}

/// An FFmpeg filter_complex fragment from `input` to `output` that pixelates the regions:
/// just their boxes ("boxes") or the whole frame ("frame"). Times are on the input's timeline.
pub fn video_filter(regions: &[Region], style: &str, input: &str, output: &str) -> String {
    if regions.is_empty() {
        return format!("{input}null{output}");
    }
    // Each box filters every frame of the video, so a long list falls back to whole frames.
    if style == "frame" || regions.len() > MAX_BOXES {
        let when: Vec<String> = regions.iter().map(|r| format!("between(t,{:.2},{:.2})", r.start, r.end)).collect();
        return format!("{input}pixelize=w=48:h=48:enable='{}'{output}", when.join("+"));
    }
    let mut g = String::new();
    let mut cur = input.to_string();
    for (i, r) in regions.iter().enumerate() {
        let [x, y, w, h] = r.bbox;
        let next = if i + 1 == regions.len() { output.to_string() } else { format!("[nz{i}]") };
        // Block size scales with the box, so small and large boxes are both unreadable.
        g.push_str(&format!(
            "{cur}split[nb{i}][nc{i}];[nc{i}]crop=w=iw*{w:.4}:h=ih*{h:.4}:x=iw*{x:.4}:y=ih*{y:.4},pixelize=w=24:h=24[np{i}];\
             [nb{i}][np{i}]overlay=x=W*{x:.4}:y=H*{y:.4}:enable='between(t,{:.2},{:.2})'{next};",
            r.start, r.end
        ));
        cur = next;
    }
    g.pop();
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(t: f64, bbox: [f64; 4]) -> Hit {
        Hit { t, score: 0.9, bbox }
    }

    #[test]
    fn decode_maps_boxes_to_frame_fractions() {
        // One anchor: a 32x18 box centred at (160, 90) in a 16:9 frame (content 320x180).
        let anchors = 1;
        let mut out = vec![0f32; 22];
        out[0] = 160.0;
        out[1] = 90.0;
        out[2] = 32.0;
        out[3] = 18.0;
        out[4 + 3] = 0.8; // FEMALE_BREAST_EXPOSED
        let h = decode(&out, anchors, 16.0 / 9.0, 1.5, 0.5);
        assert_eq!(h.len(), 1);
        let b = h[0].bbox;
        assert!((b[0] - 0.45).abs() < 1e-6 && (b[1] - 0.45).abs() < 1e-6, "{b:?}");
        assert!((b[2] - 0.1).abs() < 1e-6 && (b[3] - 0.1).abs() < 1e-6, "{b:?}");
        assert_eq!(h[0].t, 1.5);
    }

    #[test]
    fn decode_ignores_covered_and_weak_classes() {
        let mut out = vec![0f32; 22];
        out[2] = 10.0;
        out[3] = 10.0;
        out[4 + 16] = 0.9; // FEMALE_BREAST_COVERED
        assert!(decode(&out, 1, 1.0, 0.0, 0.5).is_empty());
        out[4 + 16] = 0.0;
        out[4 + 2] = 0.4; // exposed buttocks, under the threshold
        assert!(decode(&out, 1, 1.0, 0.0, 0.5).is_empty());
    }

    #[test]
    fn overlapping_boxes_keep_the_strongest() {
        let a = Hit { t: 0.0, score: 0.9, bbox: [0.1, 0.1, 0.2, 0.2] };
        let b = Hit { t: 0.0, score: 0.7, bbox: [0.11, 0.11, 0.2, 0.2] };
        let c = Hit { t: 0.0, score: 0.6, bbox: [0.6, 0.6, 0.2, 0.2] };
        assert_eq!(nms(vec![b, a, c], 0.45), vec![a, c]);
    }

    #[test]
    fn single_frame_hits_are_dropped() {
        assert!(regions(&[hit(10.0, [0.1, 0.1, 0.1, 0.1])]).is_empty());
        // Two hits in one frame are still one frame.
        assert!(regions(&[hit(10.0, [0.1, 0.1, 0.1, 0.1]), hit(10.0, [0.5, 0.5, 0.1, 0.1])]).is_empty());
    }

    #[test]
    fn nearby_hits_merge_into_one_padded_region() {
        let r = regions(&[hit(10.0, [0.4, 0.4, 0.1, 0.1]), hit(10.5, [0.45, 0.45, 0.1, 0.1]), hit(30.0, [0.0, 0.0, 0.1, 0.1]), hit(30.5, [0.0, 0.0, 0.1, 0.1])]);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].start, r[0].end), (9.5, 11.0));
        let b = r[0].bbox;
        assert!(b[0] < 0.4 && b[1] < 0.4 && b[0] + b[2] > 0.55 && b[1] + b[3] > 0.55, "{b:?}");
        assert!(r[1].bbox[0] >= 0.0 && r[1].bbox[1] >= 0.0);
    }

    #[test]
    fn regions_move_onto_a_clip_timeline() {
        let r = [Region { start: 5.0, end: 8.0, bbox: [0.0; 4] }, Region { start: 50.0, end: 52.0, bbox: [0.0; 4] }];
        let s = shift(&r, |t| (6.0..=36.0).contains(&t).then_some(t - 6.0), 30.0);
        assert_eq!(s, vec![Region { start: 0.0, end: 2.0, bbox: [0.0; 4] }]);
    }

    #[test]
    fn short_scans_are_not_trusted() {
        assert!(enough_frames(120, 120.0));
        assert!(enough_frames(108, 120.0));
        assert!(!enough_frames(107, 120.0));
        assert!(!enough_frames(0, 120.0));
        assert!(enough_frames(0, 1.0), "one expected frame may be lost to seeking");
        assert!(enough_frames(0, 0.0), "unknown length: no count to check");
    }

    #[test]
    fn cache_name_follows_the_source_file() {
        let dir = tempfile::tempdir().unwrap();
        let v = dir.path().join("a.mp4");
        std::fs::write(&v, b"one").unwrap();
        let v = v.to_string_lossy().to_string();
        let a = cache_name(&v, 0.5);
        assert!(a.starts_with("nudity_0.50_") && a.ends_with(".json"), "{a}");
        assert_eq!(a, cache_name(&v, 0.5));
        assert_ne!(a, cache_name(&v, 0.6));
        std::fs::write(&v, b"a longer file").unwrap();
        assert_ne!(a, cache_name(&v, 0.5), "a replaced file needs a new scan");
        let w = dir.path().join("b.mp4").to_string_lossy().to_string();
        assert_ne!(cache_name(&w, 0.5), cache_name(&v, 0.5));
    }

    #[test]
    fn filters_chain_from_input_to_output() {
        let r = [Region { start: 1.0, end: 2.0, bbox: [0.1, 0.2, 0.3, 0.4] }, Region { start: 5.0, end: 6.0, bbox: [0.5, 0.5, 0.2, 0.2] }];
        let g = video_filter(&r, "boxes", "[0:v]", "[nv]");
        assert!(g.starts_with("[0:v]split") && g.ends_with("[nv]"), "{g}");
        assert_eq!(g.matches("overlay").count(), 2);
        let f = video_filter(&r, "frame", "[0:v]", "[nv]");
        assert_eq!(f, "[0:v]pixelize=w=48:h=48:enable='between(t,1.00,2.00)+between(t,5.00,6.00)'[nv]");
        assert_eq!(video_filter(&[], "boxes", "[0:v]", "[nv]"), "[0:v]null[nv]");
    }
}

#[cfg(test)]
mod live {
    /// Manual end-to-end check: NUDE_MODEL, NUDE_VIDEO, NUDE_OUT env vars.
    #[tokio::test]
    #[ignore]
    async fn scan_and_pixelate_real_video() {
        let model = std::path::PathBuf::from(std::env::var("NUDE_MODEL").unwrap());
        let video = std::env::var("NUDE_VIDEO").unwrap();
        let out = std::env::var("NUDE_OUT").unwrap();
        let hits = super::scan(&model, &video, 520.0, 15.0, 16.0 / 9.0, 0.5, |p| println!("progress {p:.2}")).await.unwrap();
        let regions = super::regions(&hits);
        println!("hits={} regions={:?}", hits.len(), regions);
        // This stream has no nudity, so the render path gets a hand-made region over the cam.
        let fake = [super::Region { start: 2.0, end: 10.0, bbox: [0.0, 0.38, 0.3, 0.32] }];
        let graph = super::video_filter(&fake, "boxes", "[0:v]", "[v]");
        let st = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-ss", "520", "-t", "15", "-i", &video, "-filter_complex", &graph, "-map", "[v]", "-an", &out])
            .status().unwrap();
        assert!(st.success());
    }
}
