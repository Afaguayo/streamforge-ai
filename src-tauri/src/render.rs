//! Clip rendering with FFmpeg: 9:16 layout, animated word-by-word captions (ASS),
//! loudness normalisation, NVENC or x264. Also vertical covers and 1280×720 thumbnails.
//!
//! Subtitles are written to a work folder and FFmpeg runs from there, so the
//! `subtitles=` filter never needs Windows path escaping.

use crate::{analysis::Word, censor, nudity};
use crate::media::hidden;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct Layout {
    pub kind: String,          // blur | facecam
    pub facecam: [f64; 4],     // x, y, w, h as fractions of the source
}

/// Gameplay in the blurred layout fits a 1080x608 box, centred 110 px above the middle, so its
/// bottom edge is never below this.
const BLUR_VIDEO_BOTTOM: u32 = (1920 + 608) / 2 - 110;
/// The webcam layout, top to bottom: a band for the hook, the webcam, the gameplay, then the
/// band for captions. Every text has its own band, so none covers a face or the game.
const HOOK_BAND: u32 = 220;
const CAM_H: u32 = 460;
const GAME_H: u32 = 740;

impl Layout {
    /// Where captions start: just under the video. They're top-aligned, so a phrase that
    /// wraps grows down into the blurred band, never up over the video.
    pub fn caption_top(&self) -> u32 {
        let video_bottom = if self.kind == "facecam" { HOOK_BAND + CAM_H + GAME_H } else { BLUR_VIDEO_BOTTOM };
        video_bottom + 40
    }

    /// Where the opening hook goes: above the video in both layouts.
    pub fn hook_top(&self) -> u32 {
        if self.kind == "facecam" { 40 } else { 250 }
    }
}

// ---------- ASS captions ----------

fn ts(t: f64) -> String {
    let t = t.max(0.0);
    let cs = (t * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
}

fn clean(t: &str) -> String {
    t.replace(['{', '}', '\\'], "").trim().to_uppercase()
}

/// Caption colour per speaker, in ASS order (blue, green, red). The UI uses the same colours.
const SPEAKER_COLOURS: [&str; 6] = ["FFFFFF", "FFD84F", "B87AFF", "6BFF9C", "4DB8FF", "FFA2C9"];

const ASS_HEAD: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: {W}\nPlayResY: {H}\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Cap,Arial Black,80,&H00FFFFFF,&H000000FF,&H00101010,&H78000000,-1,0,0,0,100,100,0,0,1,7,3,8,90,90,{CAPV},1\n\
Style: Hook,Arial Black,62,&H00FFFFFF,&H000000FF,&H00101010,&HD0101010,-1,0,0,0,100,100,0,0,3,22,0,8,90,90,{HOOKV},1\n\
Style: Thumb,Arial Black,120,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,12,6,1,60,60,60,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

/// Groups words into short phrases (≤3 words, split at pauses and sentence ends).
pub fn phrases(words: &[Word]) -> Vec<Vec<&Word>> {
    let mut out: Vec<Vec<&Word>> = Vec::new();
    for w in words {
        let start_new = match out.last() {
            None => true,
            Some(p) => {
                let last = p.last().unwrap();
                p.len() >= 3 || w.sp != last.sp || w.s - last.e > 0.45 || last.t.ends_with(['.', '!', '?', ','])
            }
        };
        if start_new {
            out.push(vec![w]);
        } else {
            out.last_mut().unwrap().push(w);
        }
    }
    out
}

/// Word-by-word karaoke captions for a clip starting at `clip_start` (times become clip-relative).
pub fn captions_ass(words: &[Word], clip_start: f64, clip_end: f64, hook: Option<&str>, caption_top: u32) -> String {
    captions_ass_at(words, clip_start, clip_end, hook, caption_top, 250)
}

pub fn captions_ass_at(words: &[Word], clip_start: f64, clip_end: f64, hook: Option<&str>, caption_top: u32, hook_top: u32) -> String {
    let mut ass = ASS_HEAD.replace("{W}", "1080").replace("{H}", "1920").replace("{CAPV}", &caption_top.to_string()).replace("{HOOKV}", &hook_top.to_string());
    if let Some(h) = hook.filter(|h| !h.trim().is_empty()) {
        ass.push_str(&format!("Dialogue: 1,{},{},Hook,,0,0,0,,{{\\fad(150,250)}}{}\n", ts(0.0), ts(3.2), clean(h)));
    }
    // Bleeped words aren't shown at all, not even masked.
    let inside: Vec<Word> = words
        .iter()
        .filter(|w| !w.b && w.s >= clip_start && w.s < clip_end)
        .map(|w| Word { s: w.s - clip_start, e: (w.e - clip_start).min(clip_end - clip_start), t: w.t.clone(), sp: w.sp, b: false })
        .collect();
    let groups = phrases(&inside);
    for (gi, p) in groups.iter().enumerate() {
        // A phrase never outlasts the start of the next one, or libass stacks two lines.
        let next_phrase = groups.get(gi + 1).map(|n| n[0].s).unwrap_or(f64::MAX);
        for (i, w) in p.iter().enumerate() {
            let end = p.get(i + 1).map(|n| n.s).unwrap_or((w.e + 0.25).min(next_phrase)).max(w.s + 0.08);
            // Speaker 0 keeps the white style; `\r` resets colour, so it's set again after it.
            let colour = match w.sp as usize % SPEAKER_COLOURS.len() {
                0 => String::new(),
                n => format!("{{\\c&H00{}&}}", SPEAKER_COLOURS[n]),
            };
            let text: Vec<String> = p
                .iter()
                .enumerate()
                .map(|(j, x)| if j == i { format!("{{\\c&H0000E5FF&\\fscx128\\fscy128\\t(0,140,\\fscx112\\fscy112)}}{}{{\\r}}{colour}", clean(&x.t)) } else { clean(&x.t) })
                .collect();
            // The spoken word pops (above: big, then settling); each new phrase fades in.
            let enter = if i == 0 { "{\\fad(90,0)}" } else { "" };
            ass.push_str(&format!("Dialogue: 0,{},{},Cap,,0,0,0,,{enter}{colour}{}\n", ts(w.s), ts(end), text.join(" ")));
        }
    }
    ass
}

/// Big headline text for a still image (cover or thumbnail), shown for 10 s from t=0.
fn still_ass(w: u32, h: u32, text: &str, style: &str, align_override: &str) -> String {
    // The caption and hook positions aren't used by stills, but must still be valid numbers.
    let mut ass = ASS_HEAD.replace("{W}", &w.to_string()).replace("{H}", &h.to_string()).replace("{CAPV}", "640").replace("{HOOKV}", "250");
    // `\\q0` wraps long headlines instead of running them off the frame.
    ass.push_str(&format!("Dialogue: 0,{},{},{style},,0,0,0,,{{\\q0}}{align_override}{}\n", ts(0.0), ts(10.0), clean(text)));
    ass
}

// ---------- filter graphs ----------

pub fn vertical_graph(layout: &Layout, subs: bool) -> String {
    let base = if layout.kind == "facecam" {
        let [x, y, w, h] = layout.facecam;
        format!(
            "[0:v]split=3[c][g][b];[b]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,gblur=sigma=28,eq=brightness=-0.10:saturation=1.1[bg];\
[c]crop=iw*{w}:ih*{h}:iw*{x}:ih*{y},scale=1080:{CAM_H}:force_original_aspect_ratio=increase,crop=1080:{CAM_H},setsar=1[cam];\
[g]crop='min(iw,ih*1080/{GAME_H})':ih,scale=1080:{GAME_H},setsar=1[game];[bg][cam]overlay=0:{HOOK_BAND}[t];[t][game]overlay=0:{}+{CAM_H},setsar=1[base]", HOOK_BAND
        )
    } else {
        "[0:v]split=2[v1][v2];[v1]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,gblur=sigma=28,eq=brightness=-0.10:saturation=1.1[bg];\
[v2]scale=1080:608:force_original_aspect_ratio=decrease,setsar=1[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2-110,setsar=1[base]"
            .to_string()
    };
    if subs { format!("{base};[base]subtitles=subs.ass[out]") } else { format!("{base};[base]null[out]") }
}

fn encoder_args(nvenc: bool) -> Vec<&'static str> {
    if nvenc {
        vec!["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", "21", "-b:v", "0", "-profile:v", "high"]
    } else {
        vec!["-c:v", "libx264", "-preset", "medium", "-crf", "20"]
    }
}

/// Runs FFmpeg at below-normal priority from `cwd`, reporting progress over `duration` seconds.
pub async fn run_ffmpeg(cwd: &Path, args: &[String], duration: f64, mut progress: impl FnMut(f64)) -> Result<()> {
    let mut cmd = hidden("ffmpeg");
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000 | 0x0000_4000); // no window + BELOW_NORMAL_PRIORITY_CLASS
    let mut child = cmd
        .current_dir(cwd)
        .args(["-y", "-v", "error", "-nostats", "-progress", "pipe:1"])
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("couldn't start ffmpeg")?;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        while let Some(l) = lines.next_line().await? {
            if let Some(us) = l.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                if duration > 0.0 {
                    progress((us / 1e6 / duration).clamp(0.0, 1.0));
                }
            }
        }
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("ffmpeg failed: {}", err.lines().rev().take(3).collect::<Vec<_>>().join(" | ")));
    }
    Ok(())
}

pub struct ClipJob<'a> {
    pub video: &'a str,
    pub start: f64,
    pub end: f64,
    pub words: &'a [Word],
    pub hook: Option<&'a str>,
    pub layout: Layout,
    pub subtitles: bool,
    pub nvenc: bool,
    pub work: PathBuf,
    pub out: PathBuf,
    /// Bleeped ranges in recording time.
    pub censors: &'a [(f64, f64)],
    pub censor_style: &'a str,
    /// Pixelated areas on the clip's own timeline.
    pub nudity: &'a [nudity::Region],
    pub nudity_style: &'a str,
}

/// On-screen areas to pixelate in a full-stream copy, in recording time. With none, the
/// video is copied untouched; with some, it has to be re-encoded.
pub struct Pixelate<'a> {
    pub regions: &'a [nudity::Region],
    pub style: &'a str,
    pub nvenc: bool,
}

impl Pixelate<'_> {
    pub const NONE: Pixelate<'static> = Pixelate { regions: &[], style: "boxes", nvenc: false };
}

/// Runs `graph` (which reads `[0:v]`) on a pixelated copy of the video when there are regions.
fn pixelated(graph: String, regions: &[nudity::Region], style: &str) -> String {
    if regions.is_empty() {
        return graph;
    }
    format!("{};{}", nudity::video_filter(regions, style, "[0:v]", "[nv]"), graph.replace("[0:v]", "[nv]"))
}

const CLIP_AUDIO: &str = "highpass=f=70,loudnorm=I=-14:TP=-1.5:LRA=11";

/// Ranges on the clip's own timeline, keeping only those inside it.
pub fn clip_ranges(censors: &[(f64, f64)], start: f64, end: f64) -> Vec<(f64, f64)> {
    censors.iter().filter(|(a, b)| *b > start && *a < end).map(|(a, b)| ((a - start).max(0.0), (b - start).min(end - start))).collect()
}

pub async fn render_clip(j: ClipJob<'_>, progress: impl FnMut(f64)) -> Result<()> {
    std::fs::create_dir_all(&j.work)?;
    std::fs::create_dir_all(j.out.parent().unwrap())?;
    std::fs::write(j.work.join("subs.ass"), captions_ass_at(j.words, j.start, j.end, j.hook, j.layout.caption_top(), j.layout.hook_top()))?;
    let dur = j.end - j.start;
    let ranges = clip_ranges(j.censors, j.start, j.end);
    let mut graph = pixelated(vertical_graph(&j.layout, j.subtitles), j.nudity, j.nudity_style);
    if !ranges.is_empty() {
        // Bleeped after loudnorm, which would otherwise pull the silenced word back up.
        graph.push(';');
        graph.push_str(&censor::audio_graph("[0:a:0]", &format!("{CLIP_AUDIO},aresample=48000,"), &ranges, j.censor_style, dur));
    }
    let audio_map = if ranges.is_empty() { "0:a:0?" } else { "[aout]" };
    let mut args: Vec<String> = vec![
        "-ss".into(), format!("{:.3}", j.start), "-i".into(), j.video.into(), "-t".into(), format!("{dur:.3}"),
        "-filter_complex".into(), graph, "-map".into(), "[out]".into(), "-map".into(), audio_map.into(),
        "-fpsmax".into(), "60".into(), "-pix_fmt".into(), "yuv420p".into(),
    ];
    args.extend(encoder_args(j.nvenc).into_iter().map(String::from));
    if ranges.is_empty() {
        args.extend(["-af".to_string(), CLIP_AUDIO.to_string()]);
    }
    args.extend(["-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-movflags", "+faststart"].map(String::from));
    args.push(j.out.to_string_lossy().into());
    run_ffmpeg(&j.work, &args, dur, progress).await
}

/// Copies a full recording with bleeped audio. The video is copied untouched, so this takes
/// minutes rather than hours.
pub async fn censor_full_video(video: &str, out: &Path, ranges: &[(f64, f64)], style: &str, pix: &Pixelate<'_>, dur: f64, progress: impl FnMut(f64)) -> Result<()> {
    let dir = out.parent().ok_or_else(|| anyhow!("no output folder"))?;
    std::fs::create_dir_all(dir)?;
    // A name of its own per run: if the app is force-closed mid-job, FFmpeg can keep writing
    // its file, and a retry writing the same file interleaves the two into a broken video.
    let part = out.with_extension(format!("{}.part.mp4", std::process::id()));
    let mut args: Vec<String> = vec!["-i".into(), video.into()];
    args.extend(full_video_maps(ranges, style, pix, dur));
    args.extend(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"].map(String::from));
    args.push(part.to_string_lossy().into());
    run_ffmpeg(dir, &args, dur, progress).await?;
    std::fs::rename(&part, out)?;
    Ok(())
}

/// Filter and stream-mapping arguments for a full-stream copy: bleeped audio when there are
/// `ranges`, a pixelated, re-encoded picture when there are regions, and copies otherwise.
fn full_video_maps(ranges: &[(f64, f64)], style: &str, pix: &Pixelate<'_>, dur: f64) -> Vec<String> {
    let mut graph = Vec::new();
    if !pix.regions.is_empty() {
        graph.push(nudity::video_filter(pix.regions, pix.style, "[0:v:0]", "[vout]"));
    }
    if !ranges.is_empty() {
        graph.push(censor::audio_graph("[0:a:0]", "aresample=48000,", ranges, style, dur));
    }
    let mut args = Vec::new();
    if !graph.is_empty() {
        args.extend(["-filter_complex".to_string(), graph.join(";")]);
    }
    args.extend(["-map".to_string(), if pix.regions.is_empty() { "0:v:0" } else { "[vout]" }.to_string()]);
    args.extend(["-map".to_string(), if ranges.is_empty() { "0:a:0" } else { "[aout]" }.to_string()]);
    if pix.regions.is_empty() {
        args.extend(["-c:v".to_string(), "copy".to_string()]);
    } else {
        args.extend(["-pix_fmt", "yuv420p"].map(String::from));
        args.extend(encoder_args(pix.nvenc).into_iter().map(String::from));
    }
    args
}

/// Keyframe times of the video stream, read from packets (no decoding).
pub async fn keyframes(video: &str) -> Result<Vec<f64>> {
    let out = hidden("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0", video])
        .output()
        .await?;
    if !out.status.success() {
        return Err(anyhow!("couldn't read keyframes: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let mut k: Vec<f64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (t, flags) = l.split_once(',')?;
            flags.starts_with('K').then(|| t.parse().ok())?
        })
        .collect();
    k.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(k)
}

/// The parts of a video kept after `cuts`, widened to keyframes so the video can be copied
/// without re-encoding. Widening only ever keeps a little more, never less.
pub fn kept_parts(duration: f64, cuts: &[(f64, f64)], keys: &[f64]) -> Vec<(f64, f64)> {
    let mut parts = Vec::new();
    let mut at = 0.0;
    for (a, b) in cuts {
        if *a > at {
            parts.push((at, *a));
        }
        at = at.max(*b);
    }
    if duration > at {
        parts.push((at, duration));
    }
    let down = |t: f64| keys.iter().copied().filter(|k| *k <= t + 1e-6).last().unwrap_or(0.0);
    let up = |t: f64| keys.iter().copied().find(|k| *k >= t - 1e-6).unwrap_or(duration);
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in parts {
        let (a, b) = (down(a), if b >= duration { duration } else { up(b) });
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ if b > a => out.push((a, b)),
            _ => {}
        }
    }
    out
}

/// A time in the original video, on the timeline of the kept parts.
pub fn remap(t: f64, parts: &[(f64, f64)]) -> Option<f64> {
    let mut before = 0.0;
    for (a, b) in parts {
        if t < *a {
            return None;
        }
        if t <= *b {
            return Some(before + t - a);
        }
        before += b - a;
    }
    None
}

/// Copies only the kept parts (video untouched, audio re-encoded with any bleeps).
pub async fn edit_full_video(video: &str, out: &Path, parts: &[(f64, f64)], censors: &[(f64, f64)], style: &str, pix: &Pixelate<'_>, progress: impl FnMut(f64)) -> Result<()> {
    let dir = out.parent().ok_or_else(|| anyhow!("no output folder"))?;
    std::fs::create_dir_all(dir)?;
    let part = out.with_extension(format!("{}.part.mp4", std::process::id()));
    let list = out.with_extension(format!("{}.txt", std::process::id()));
    let src = video.replace('\\', "/").replace('\'', "'\\''");
    let mut text = String::from("ffconcat version 1.0\n");
    for (i, (a, b)) in parts.iter().enumerate() {
        text.push_str(&format!("file '{src}'\ninpoint {a:.3}\n"));
        if i + 1 < parts.len() {
            // Just before the keyframe: ending exactly on it puts that frame in both pieces,
            // which gives two packets the same timestamp at the seam.
            text.push_str(&format!("outpoint {:.3}\n", b - 0.001));
        } else {
            text.push_str(&format!("outpoint {b:.3}\n"));
        }
    }
    std::fs::write(&list, text)?;
    let total: f64 = parts.iter().map(|(a, b)| b - a).sum();
    let ranges: Vec<(f64, f64)> = censors.iter().filter_map(|(a, b)| Some((remap(*a, parts)?, remap(*b, parts).unwrap_or(remap(*a, parts)? + (b - a))))).collect();
    let regions = nudity::shift(pix.regions, |t| remap(t, parts), total);
    let pix = Pixelate { regions: &regions, style: pix.style, nvenc: pix.nvenc };
    let mut args: Vec<String> = vec!["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list.to_string_lossy().into()];
    args.extend(full_video_maps(&ranges, style, &pix, total));
    args.extend(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"].map(String::from));
    args.push(part.to_string_lossy().into());
    let res = run_ffmpeg(dir, &args, total, progress).await;
    let _ = std::fs::remove_file(&list);
    res?;
    std::fs::rename(&part, out)?;
    Ok(())
}

fn srt_ts(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// A subtitle track (SRT) for a full stream as uploaded: lines of up to 10 words, times moved
/// onto the timeline of the kept `parts`, bleeped words left out.
pub fn subtitles_srt(words: &[Word], parts: &[(f64, f64)]) -> String {
    let mut out = String::new();
    let mut n = 0;
    for l in crate::analysis::lines_of(words, 10) {
        let (Some(a), Some(b)) = (remap(l.s, parts), remap(l.e, parts).or_else(|| remap(l.s, parts).map(|a| a + (l.e - l.s).min(5.0)))) else { continue };
        n += 1;
        out.push_str(&format!("{n}\n{} --> {}\n{}\n\n", srt_ts(a), srt_ts(b.max(a + 0.5)), l.text));
    }
    out
}

/// Headline text over a finished image (the AI thumbnail), 1280x720.
pub async fn text_over_image(image: &Path, text: &str, work: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(work)?;
    std::fs::create_dir_all(out.parent().ok_or_else(|| anyhow!("no output folder"))?)?;
    let (_, _, ov) = thumbnail_concepts()[1];
    std::fs::write(work.join("subs.ass"), still_ass(1280, 720, text, "Thumb", ov))?;
    let args: Vec<String> = vec![
        "-i".into(), image.to_string_lossy().into(), "-frames:v".into(), "1".into(),
        "-vf".into(), "scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,subtitles=subs.ass".into(),
        out.to_string_lossy().into(),
    ];
    run_ffmpeg(work, &args, 0.0, |_| {}).await
}

/// One plain 1280x720 frame (no text), the input for an AI thumbnail.
pub async fn plain_frame(video: &str, at: f64, out: &Path) -> Result<()> {
    let dir = out.parent().ok_or_else(|| anyhow!("no output folder"))?;
    std::fs::create_dir_all(dir)?;
    let args: Vec<String> = vec![
        "-ss".into(), format!("{:.3}", (at - 2.0).max(0.0)), "-t".into(), "4".into(), "-i".into(), video.into(), "-frames:v".into(), "1".into(),
        "-vf".into(), format!("fps=15,{EDGE_CROP},scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,thumbnail=50"), "-q:v".into(), "2".into(),
        out.to_string_lossy().into(),
    ];
    run_ffmpeg(dir, &args, 0.0, |_| {}).await
}

/// One frame at `at`, laid out like the clip, with the hook in large type.
pub async fn render_cover(video: &str, at: f64, text: &str, layout: &Layout, blur: bool, work: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(work)?;
    std::fs::create_dir_all(out.parent().ok_or_else(|| anyhow!("no output folder"))?)?;
    std::fs::write(work.join("subs.ass"), still_ass(1080, 1920, text, "Hook", "{\\an5\\fs86}"))?;
    let whole = [nudity::Region { start: 0.0, end: 1e9, bbox: [0.0, 0.0, 1.0, 1.0] }];
    let regions: &[nudity::Region] = if blur { &whole } else { &[] };
    let args: Vec<String> = vec![
        "-ss".into(), format!("{at:.3}"), "-i".into(), video.into(), "-frames:v".into(), "1".into(),
        "-filter_complex".into(), pixelated(vertical_graph(layout, true), regions, "frame"),
        "-map".into(), "[out]".into(), "-q:v".into(), "3".into(), out.to_string_lossy().into(),
    ];
    run_ffmpeg(work, &args, 0.0, |_| {}).await
}

/// Average brightness (0-255) of an image. Loading screens and fades come out near black.
pub async fn brightness(image: &Path) -> Result<f64> {
    let out = hidden("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(image)
        .args(["-vf", "signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=-", "-f", "null", "-"])
        .output()
        .await?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.split("YAVG=").nth(1)?.trim().parse().ok())
        .ok_or_else(|| anyhow!("couldn't measure brightness"))
}

/// Trims the frame edges, where stream overlays (channel name, chat, HUD) and burned-in game
/// subtitles sit, so a thumbnail shows only the action.
const EDGE_CROP: &str = "crop=iw*0.76:ih*0.72:iw*0.12:ih*0.13";

/// Three thumbnail concepts, each a different composition of a strong frame + headline.
pub fn thumbnail_concepts() -> [(&'static str, &'static str, &'static str); 3] {
    [
        // (name, extra video filter, ASS override)
        ("Bold bottom-left headline", "eq=contrast=1.18:saturation=1.35", "{\\an1\\c&H00FFFFFF&\\3c&H00000000&}"),
        ("Punch-in with yellow headline", "crop=iw/1.2:ih/1.2,scale=1280:720,eq=contrast=1.2:saturation=1.4", "{\\an7\\c&H0000E5FF&\\3c&H00000000&\\fs110}"),
        ("Centered statement", "eq=contrast=1.12:brightness=0.03:saturation=1.3", "{\\an5\\fs115\\c&H00FFFFFF&\\3c&H002020E0&}"),
    ]
}

pub async fn render_thumbnail(video: &str, at: f64, text: &str, concept: usize, work: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(work)?;
    // Thumbnails can run before any clip has made the stream's output folder.
    std::fs::create_dir_all(out.parent().ok_or_else(|| anyhow!("no output folder"))?)?;
    let (_, vf, ov) = thumbnail_concepts()[concept % 3];
    std::fs::write(work.join("subs.ass"), still_ass(1280, 720, text, "Thumb", ov))?;
    let args: Vec<String> = vec![
        // `thumbnail` picks the clearest frame of the 4 s around the moment, not a mid-motion blur.
        "-ss".into(), format!("{:.3}", (at - 2.0).max(0.0)), "-t".into(), "4".into(), "-i".into(), video.into(), "-frames:v".into(), "1".into(),
        "-vf".into(), format!("fps=15,{EDGE_CROP},scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,thumbnail=50,{vf},subtitles=subs.ass"),
        out.to_string_lossy().into(),
    ];
    run_ffmpeg(work, &args, 0.0, |_| {}).await
}

/// Up to seven words from a title for thumbnail text, filler dropped and "!!!!" cut to "!".
pub fn headline(title: &str) -> String {
    let stop = ["the", "a", "an", "of", "to", "el", "la", "los", "las", "de", "que", "y", "en", "un", "una"];
    let words: Vec<&str> = title
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '!' && c != '?'))
        .filter(|w| !w.is_empty() && !stop.contains(&w.to_lowercase().as_str()))
        .collect();
    let mut text = words.iter().take(7).copied().collect::<Vec<_>>().join(" ");
    while text.contains("!!") { text = text.replace("!!", "!"); }
    while text.contains("??") { text = text.replace("??", "?"); }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: f64, t: &str) -> Word {
        Word { s, e: s + 0.3, t: t.into(), sp: 0, b: false }
    }

    #[test]
    fn timestamps_are_ass_format() {
        assert_eq!(ts(3723.456), "1:02:03.46");
        assert_eq!(ts(0.0), "0:00:00.00");
    }

    #[test]
    fn phrases_split_on_pause_punctuation_and_length() {
        let words = [w(0.0, "uno"), w(0.35, "dos."), w(0.7, "tres"), w(1.0, "cuatro"), w(1.3, "cinco"), w(1.6, "seis"), w(3.0, "siete")];
        let p = phrases(&words);
        let sizes: Vec<usize> = p.iter().map(|x| x.len()).collect();
        assert_eq!(sizes, vec![2, 3, 1, 1]);
    }

    #[test]
    fn captions_are_clip_relative_and_highlight_one_word() {
        let words = [w(99.0, "antes"), w(100.5, "hola"), w(100.9, "mundo"), w(200.0, "despues")];
        let ass = captions_ass(&words, 100.0, 160.0, Some("no lo vas a creer"), 1194);
        assert!(ass.contains("Hook,,0,0,0,,{\\fad(150,250)}NO LO VAS A CREER"));
        assert!(ass.contains("Dialogue: 0,0:00:00.50,0:00:00.90,Cap,,0,0,0,,{\\fad(90,0)}{\\c&H0000E5FF&\\fscx128\\fscy128\\t(0,140,\\fscx112\\fscy112)}HOLA{\\r} MUNDO"), "{ass}");
        assert!(ass.contains("HOLA {\\c&H0000E5FF&"), "the second word pops without a new fade");
        assert!(ass.contains(",8,90,90,1194,1"), "captions top-aligned under the video");
        assert!(!ass.contains("ANTES") && !ass.contains("DESPUES"));
    }

    #[test]
    fn caption_events_never_overlap() {
        let words: Vec<Word> = (0..12).map(|i| Word { s: i as f64 * 0.5, e: i as f64 * 0.5 + 0.3, t: "w".into(), sp: 0, b: false }).collect();
        let ass = captions_ass(&words, 0.0, 60.0, None, 1194);
        let mut spans: Vec<(String, String)> = ass
            .lines()
            .filter(|l| l.starts_with("Dialogue: 0,"))
            .map(|l| { let f: Vec<&str> = l.split(',').collect(); (f[1].to_string(), f[2].to_string()) })
            .collect();
        spans.sort();
        for pair in spans.windows(2) {
            assert!(pair[0].1 <= pair[1].0, "{:?} overlaps {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn speakers_get_their_own_colour_and_phrases() {
        let mut words = [w(1.0, "hola"), w(1.3, "que"), w(1.6, "tal")];
        words[2].sp = 1;
        let ass = captions_ass(&words, 0.0, 60.0, None, 1194);
        let caps: Vec<&str> = ass.lines().filter(|l| l.starts_with("Dialogue: 0,")).collect();
        assert_eq!(caps.len(), 3);
        assert!(!caps[0].contains("&H00FFD84F&"), "first speaker stays white");
        assert!(caps[2].contains("{\\c&H00FFD84F&}"), "{}", caps[2]);
        assert!(!caps[2].contains("HOLA"), "a new speaker starts a new phrase");
    }

    #[test]
    fn bleeped_words_are_left_out_of_captions() {
        let mut words = [w(1.0, "hey"), w(1.3, "secret"), w(1.6, "there")];
        words[1].b = true;
        let ass = captions_ass(&words, 0.0, 60.0, None, 1194);
        assert!(!ass.contains("SECRET") && !ass.contains("S*****") && !ass.contains('*'), "{ass}");
        assert!(ass.contains("HEY") && ass.contains("THERE"));
    }

    #[test]
    fn subtitle_track_follows_the_cuts_and_skips_bleeps() {
        let mut words = vec![w(1.0, "hello"), w(1.3, "there."), w(30.0, "gone"), w(70.0, "after"), w(70.3, "n****")];
        words[4].b = true;
        let srt = subtitles_srt(&words, &[(0.0, 10.0), (60.0, 100.0)]);
        assert!(srt.starts_with("1\n00:00:01,000 --> "), "{srt}");
        assert!(srt.contains("HELLO THERE.") || srt.contains("hello there."), "{srt}");
        assert!(!srt.contains("gone"), "words inside a cut are dropped");
        assert!(srt.contains("00:00:20,000"), "70 s lands at 20 s after the 50 s cut: {srt}");
        assert!(!srt.contains('*'));
    }

    #[test]
    fn censor_ranges_move_onto_the_clip_timeline() {
        let r = clip_ranges(&[(5.0, 6.0), (99.8, 100.4), (130.0, 131.0), (159.8, 161.0), (300.0, 301.0)], 100.0, 160.0);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].0, 0.0);
        assert!((r[1].0 - 30.0).abs() < 1e-9);
        assert_eq!(r[2].1, 60.0);
    }

    #[test]
    fn kept_parts_skip_cuts_and_widen_to_keyframes() {
        let keys: Vec<f64> = (0..=50).map(|i| i as f64 * 2.0).collect();
        let parts = kept_parts(100.0, &[(10.5, 40.5), (95.0, 100.0)], &keys);
        assert_eq!(parts, vec![(0.0, 12.0), (40.0, 96.0)], "kept more, never less");
        assert_eq!(remap(5.0, &parts), Some(5.0));
        assert_eq!(remap(20.0, &parts), None, "inside a cut");
        assert_eq!(remap(50.0, &parts), Some(22.0));
    }

    #[test]
    fn no_cuts_keeps_everything() {
        assert_eq!(kept_parts(30.0, &[], &[0.0, 10.0, 20.0]), vec![(0.0, 30.0)]);
    }

    #[test]
    fn no_template_placeholder_is_left_in_any_subtitle_file() {
        for ass in [still_ass(1280, 720, "x", "Thumb", ""), captions_ass(&[w(1.0, "a")], 0.0, 9.0, Some("h"), 1194)] {
            for p in ["{W}", "{H}", "{CAPV}", "{HOOKV}"] {
                assert!(!ass.contains(p), "{p} left in:
{ass}");
            }
        }
    }

    #[test]
    fn captions_start_below_the_video_in_both_layouts() {
        let blur = Layout { kind: "blur".into(), facecam: [0.0; 4] };
        let cam = Layout { kind: "facecam".into(), facecam: [0.0; 4] };
        assert_eq!(blur.caption_top(), 1154 + 40);
        assert_eq!(cam.caption_top(), 220 + 460 + 740 + 40);
        assert!(cam.hook_top() + 2 * 90 <= 220, "two hook lines fit above the webcam");
        assert!(cam.caption_top() + 2 * 100 < 1920, "two lines fit in the band");
    }

    #[test]
    fn graphs_end_in_out_label() {
        let blur = Layout { kind: "blur".into(), facecam: [0.0; 4] };
        let cam = Layout { kind: "facecam".into(), facecam: [0.7, 0.04, 0.28, 0.3] };
        assert!(vertical_graph(&blur, true).ends_with("subtitles=subs.ass[out]"));
        assert!(vertical_graph(&cam, false).contains("overlay=0:220+460"));
    }

    #[test]
    fn headline_drops_filler_words() {
        assert_eq!(headline("\"No mames, el jefe de la raid!\""), "No mames jefe raid!");
        assert_eq!(headline("PLAYING 1 HOUR BEFORE WORK GTA 6!!!!"), "PLAYING 1 HOUR BEFORE WORK GTA 6!");
    }
}

/// Real FFmpeg run: `cargo test -- --ignored` with STREAMFORGE_TEST_VIDEO set to a ≥70 s video.
#[cfg(test)]
mod ffmpeg_tests {
    use super::*;

    fn video() -> Option<String> {
        std::env::var("STREAMFORGE_TEST_VIDEO").ok()
    }

    async fn probe_dims(p: &Path) -> (i64, i64, f64) {
        let i = crate::media::probe(&p.to_string_lossy()).await.unwrap();
        (i.width.unwrap(), i.height.unwrap(), i.duration_s.unwrap_or(0.0))
    }

    #[tokio::test]
    #[ignore]
    async fn renders_vertical_clip_cover_and_thumbnail() {
        let Some(v) = video() else { return };
        let dir = tempfile::tempdir().unwrap();
        let words: Vec<Word> = (0..120).map(|i| Word { s: 5.0 + i as f64 * 0.5, e: 5.3 + i as f64 * 0.5, t: if i % 7 == 0 { "jajaja".into() } else { "palabra".into() }, sp: 0, b: false }).collect();
        for (kind, nvenc) in [("blur", false), ("facecam", crate::media::detect_toolchain().await.nvenc)] {
            let out = dir.path().join(format!("{kind}.mp4"));
            let mut last = 0.0;
            render_clip(
                ClipJob { video: &v, start: 10.0, end: 70.0, words: &words, hook: Some("no lo vas a creer"), layout: Layout { kind: kind.into(), facecam: [0.7, 0.04, 0.28, 0.3] }, subtitles: true, nvenc, work: dir.path().join(kind), out: out.clone(), censors: &[(20.0, 21.0)], censor_style: "beep", nudity: &[], nudity_style: "boxes" },
                |p| last = p,
            )
            .await
            .unwrap();
            let (w, h, d) = probe_dims(&out).await;
            assert_eq!((w, h), (1080, 1920), "{kind}");
            assert!((d - 60.0).abs() < 0.6, "{kind} duration {d}");
            assert!(last > 0.9, "progress reported");
        }
        let layout = Layout { kind: "blur".into(), facecam: [0.0; 4] };
        let cover = dir.path().join("cover.jpg");
        render_cover(&v, 20.0, "hook text", &layout, false, &dir.path().join("c"), &cover).await.unwrap();
        assert_eq!(probe_dims(&cover).await.0, 1080);
        let thumb = dir.path().join("thumb.png");
        render_thumbnail(&v, 20.0, "BIG MOMENT", 1, &dir.path().join("t"), &thumb).await.unwrap();
        assert!(brightness(&thumb).await.unwrap() > 40.0, "test pattern is bright");
        let nested = dir.path().join("not").join("made").join("thumb.png");
        render_thumbnail(&v, 20.0, "BIG MOMENT", 0, &dir.path().join("t2"), &nested).await.unwrap();
        assert_eq!(probe_dims(&thumb).await.0, 1280);
        let keys = keyframes(&v).await.unwrap();
        assert!(keys.len() > 2, "keyframes read");
        let parts = kept_parts(90.0, &[(20.0, 60.0)], &keys);
        let edited = dir.path().join("edited.mp4");
        edit_full_video(&v, &edited, &parts, &[(65.0, 66.0)], "beep", &Pixelate::NONE, |_| {}).await.unwrap();
        let (_, _, d) = probe_dims(&edited).await;
        let expected: f64 = parts.iter().map(|(a, b)| b - a).sum();
        assert!((d - expected).abs() < 1.0, "edited length {d} vs {expected}");
        assert!(d < 60.0, "the cut came out: {d}");
        let rms = crate::analysis::loudness_per_second(&v).await.unwrap();
        assert!(rms.len() >= 85, "one value per second: {}", rms.len());
        let loud: f64 = rms[40..48].iter().sum::<f64>() / 8.0;
        let quiet: f64 = rms[10..18].iter().sum::<f64>() / 8.0;
        assert!(loud > quiet + 6.0, "loud section detected: {loud} vs {quiet}");
        crate::analysis::scene_changes(&v).await.unwrap();
        if std::env::var("STREAMFORGE_KEEP").is_ok() {
            let keep = std::path::PathBuf::from(std::env::var("STREAMFORGE_KEEP").unwrap());
            for f in ["blur.mp4", "facecam.mp4", "cover.jpg", "thumb.png"] {
                std::fs::copy(dir.path().join(f), keep.join(f)).unwrap();
            }
        }
    }
}

/// Real speech through the whole censor. `cargo test real_censor -- --ignored --nocapture` with
/// STREAMFORGE_TEST_VIDEO (a stretch with the word in it), STREAMFORGE_TOOLS (the app's
/// tools\whisper folder) and STREAMFORGE_KEEP (where outputs go).
#[cfg(test)]
mod real_censor_tests {
    use super::*;
    use crate::{analysis, tools};

    /// Both passes, as the app runs them. `.0` is the transcript, `.1` the censor's second listen.
    async fn hear(video: &str, dir: &Path, tools_dir: &Path) -> (analysis::Heard, analysis::Heard) {
        std::fs::create_dir_all(dir).unwrap();
        let raw = dir.join("a.pcm");
        analysis::extract_audio(video, &raw).await.unwrap();
        let chunks = analysis::split_audio(&raw, &dir.join("p")).unwrap();
        let (exe, model) = (tools_dir.join("Release").join("whisper-cli.exe"), tools_dir.join("models").join("ggml-small.bin"));
        let vad = tools::vad_path(tools_dir.parent().unwrap().parent().unwrap());
        let a = analysis::run_whisper(&exe, &model, Some(&vad), &dir.join("p"), &chunks, |_| {}).await.unwrap();
        let b = analysis::run_whisper(&exe, &model, None, &dir.join("p"), &chunks, |_| {}).await.unwrap();
        (a, b)
    }

    #[tokio::test]
    #[ignore]
    async fn real_censor_removes_the_word_from_clip_and_full_video() {
        let (Ok(v), Ok(t), Ok(k)) = (std::env::var("STREAMFORGE_TEST_VIDEO"), std::env::var("STREAMFORGE_TOOLS"), std::env::var("STREAMFORGE_KEEP")) else { return };
        let (tools_dir, keep) = (PathBuf::from(t), PathBuf::from(k));
        let dur = crate::media::probe(&v).await.unwrap().duration_s.unwrap();
        let (first, second) = hear(&v, &keep.join("h0"), &tools_dir).await;
        let mut words = first.words;
        censor::apply(&mut words, &[], &first.english);
        censor::add_missed(&mut words, &second.words, &[], &second.english);
        let hits = censor::merge(words.iter().filter(|w| w.b).map(|w| censor::range(w.s, w.e)).collect());
        println!("source: {} words, {} censored at {hits:?}", words.len(), hits.len());
        assert!(!hits.is_empty(), "the excerpt must contain the word");
        let out = keep.join("clip.mp4");
        render_clip(
            ClipJob { video: &v, start: 0.0, end: dur, words: &words, hook: Some("test hook"), layout: Layout { kind: "blur".into(), facecam: [0.0; 4] }, subtitles: true, nvenc: true, work: keep.join("w"), out: out.clone(), censors: &hits, censor_style: "beep", nudity: &[], nudity_style: "boxes" },
            |_| {},
        )
        .await
        .unwrap();
        let full = keep.join("full.mp4");
        censor_full_video(&v, &full, &hits, "beep", &Pixelate::NONE, dur, |_| {}).await.unwrap();
        for (name, f) in [("clip", &out), ("full", &full)] {
            let (a, b) = hear(&f.to_string_lossy(), &keep.join(format!("h_{name}")), &tools_dir).await;
            let left: Vec<&Word> = a.words.iter().chain(&b.words).filter(|w| censor::is_slur(&w.t, &[])).collect();
            println!("{name}: heard {} + {} words, {} still the word: {left:?}", a.words.len(), b.words.len(), left.len());
            assert!(left.is_empty(), "{name}");
        }
        render_thumbnail(&v, 20.0, "PROJECTS ZOMBOID LOVE STORY", 0, &keep.join("t"), &keep.join("thumb0.png")).await.unwrap();
        render_thumbnail(&v, 40.0, "PROJECTS ZOMBOID LOVE STORY", 2, &keep.join("t"), &keep.join("thumb2.png")).await.unwrap();
    }
}

/// Cutting a real recording: `cargo test real_edit -- --ignored --nocapture` with
/// STREAMFORGE_EDIT_VIDEO (6+ minutes) and STREAMFORGE_KEEP.
#[cfg(test)]
mod real_edit_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn real_edit_cuts_and_stays_in_sync() {
        let (Ok(v), Ok(k)) = (std::env::var("STREAMFORGE_EDIT_VIDEO"), std::env::var("STREAMFORGE_KEEP")) else { return };
        let dur = crate::media::probe(&v).await.unwrap().duration_s.unwrap();
        let parts = kept_parts(dur, &[(61.3, 179.1)], &keyframes(&v).await.unwrap());
        println!("parts {parts:?}");
        let out = PathBuf::from(k).join("edited.mp4");
        edit_full_video(&v, &out, &parts, &[(200.0, 200.6)], "beep", &Pixelate::NONE, |_| {}).await.unwrap();
        let o = hidden("ffprobe").args(["-v", "error", "-show_entries", "stream=codec_type,duration", "-of", "csv=p=0"]).arg(&out).output().await.unwrap();
        println!("streams: {}", String::from_utf8_lossy(&o.stdout).trim().replace('\n', " | "));
        let expected: f64 = parts.iter().map(|(a, b)| b - a).sum();
        let got = crate::media::probe(&out.to_string_lossy()).await.unwrap().duration_s.unwrap();
        println!("expected {expected:.2} got {got:.2}");
        assert!((got - expected).abs() < 0.5);
    }
}

/// Look at captions on a real recording: `cargo test real_captions -- --ignored` with
/// STREAMFORGE_CAP_VIDEO and STREAMFORGE_KEEP; writes blur.mp4 and facecam.mp4.
#[cfg(test)]
mod real_caption_tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn real_captions_render_in_both_layouts() {
        let (Ok(v), Ok(k)) = (std::env::var("STREAMFORGE_CAP_VIDEO"), std::env::var("STREAMFORGE_KEEP")) else { return };
        let text = "yo bro check this out right now we are going to the supermarket extraordinarily quickly okay";
        let words: Vec<Word> = text.split(' ').enumerate().map(|(i, t)| Word { s: 2.0 + i as f64 * 0.45, e: 2.35 + i as f64 * 0.45, t: t.into(), sp: (i / 6) as u8 % 2, b: false }).collect();
        for kind in ["blur", "facecam"] {
            let out = PathBuf::from(&k).join(format!("{kind}.mp4"));
            render_clip(
                ClipJob { video: &v, start: 0.0, end: 12.0, words: &words, hook: Some("they did not see this coming"), layout: Layout { kind: kind.into(), facecam: [0.0, 0.33, 0.29, 0.29] }, subtitles: true, nvenc: true, work: PathBuf::from(&k).join(kind), out, censors: &[], censor_style: "beep", nudity: &[], nudity_style: "boxes" },
                |_| {},
            )
            .await
            .unwrap();
        }
    }
}
