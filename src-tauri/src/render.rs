//! Clip rendering with FFmpeg: 9:16 layout, animated word-by-word captions (ASS),
//! loudness normalisation, NVENC or x264. Also vertical covers and 1280×720 thumbnails.
//!
//! Subtitles are written to a work folder and FFmpeg runs from there, so the
//! `subtitles=` filter never needs Windows path escaping.

use crate::analysis::Word;
use crate::media::hidden;
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct Layout {
    pub kind: String,          // blur | facecam
    pub facecam: [f64; 4],     // x, y, w, h as fractions of the source
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

const ASS_HEAD: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: {W}\nPlayResY: {H}\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Cap,Arial Black,80,&H00FFFFFF,&H000000FF,&H00101010,&H78000000,-1,0,0,0,100,100,0,0,1,7,3,2,90,90,640,1\n\
Style: Hook,Arial Black,62,&H00FFFFFF,&H000000FF,&H00101010,&HD0101010,-1,0,0,0,100,100,0,0,3,22,0,8,90,90,250,1\n\
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
                p.len() >= 3 || w.s - last.e > 0.45 || last.t.ends_with(['.', '!', '?', ','])
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
pub fn captions_ass(words: &[Word], clip_start: f64, clip_end: f64, hook: Option<&str>) -> String {
    let mut ass = ASS_HEAD.replace("{W}", "1080").replace("{H}", "1920");
    if let Some(h) = hook.filter(|h| !h.trim().is_empty()) {
        ass.push_str(&format!("Dialogue: 1,{},{},Hook,,0,0,0,,{{\\fad(150,250)}}{}\n", ts(0.0), ts(3.2), clean(h)));
    }
    let inside: Vec<Word> = words
        .iter()
        .filter(|w| w.s >= clip_start && w.s < clip_end)
        .map(|w| Word { s: w.s - clip_start, e: (w.e - clip_start).min(clip_end - clip_start), t: w.t.clone() })
        .collect();
    let groups = phrases(&inside);
    for (gi, p) in groups.iter().enumerate() {
        // A phrase never outlasts the start of the next one, or libass stacks two lines.
        let next_phrase = groups.get(gi + 1).map(|n| n[0].s).unwrap_or(f64::MAX);
        for (i, w) in p.iter().enumerate() {
            let end = p.get(i + 1).map(|n| n.s).unwrap_or((w.e + 0.25).min(next_phrase)).max(w.s + 0.08);
            let text: Vec<String> = p
                .iter()
                .enumerate()
                .map(|(j, x)| if j == i { format!("{{\\c&H0000E5FF&\\fscx112\\fscy112}}{}{{\\r}}", clean(&x.t)) } else { clean(&x.t) })
                .collect();
            ass.push_str(&format!("Dialogue: 0,{},{},Cap,,0,0,0,,{}\n", ts(w.s), ts(end), text.join(" ")));
        }
    }
    ass
}

/// Big headline text for a still image (cover or thumbnail), shown for 10 s from t=0.
fn still_ass(w: u32, h: u32, text: &str, style: &str, align_override: &str) -> String {
    let mut ass = ASS_HEAD.replace("{W}", &w.to_string()).replace("{H}", &h.to_string());
    ass.push_str(&format!("Dialogue: 0,{},{},{style},,0,0,0,,{align_override}{}\n", ts(0.0), ts(10.0), clean(text)));
    ass
}

// ---------- filter graphs ----------

pub fn vertical_graph(layout: &Layout, subs: bool) -> String {
    let base = if layout.kind == "facecam" {
        let [x, y, w, h] = layout.facecam;
        format!(
            "[0:v]split=2[c][g];[c]crop=iw*{w}:ih*{h}:iw*{x}:ih*{y},scale=1080:640:force_original_aspect_ratio=increase,crop=1080:640,setsar=1[cam];\
[g]crop='min(iw,ih*1080/1280)':ih,scale=1080:1280,setsar=1[game];[cam][game]vstack=inputs=2[base]"
        )
    } else {
        "[0:v]split=2[v1][v2];[v1]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,gblur=sigma=28,eq=brightness=-0.10:saturation=1.1[bg];\
[v2]scale=1080:-2,setsar=1[fg];[bg][fg]overlay=0:(H-h)/2-110,setsar=1[base]"
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
async fn run_ffmpeg(cwd: &Path, args: &[String], duration: f64, mut progress: impl FnMut(f64)) -> Result<()> {
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
}

pub async fn render_clip(j: ClipJob<'_>, progress: impl FnMut(f64)) -> Result<()> {
    std::fs::create_dir_all(&j.work)?;
    std::fs::create_dir_all(j.out.parent().unwrap())?;
    std::fs::write(j.work.join("subs.ass"), captions_ass(j.words, j.start, j.end, j.hook))?;
    let dur = j.end - j.start;
    let mut args: Vec<String> = vec![
        "-ss".into(), format!("{:.3}", j.start), "-i".into(), j.video.into(), "-t".into(), format!("{dur:.3}"),
        "-filter_complex".into(), vertical_graph(&j.layout, j.subtitles), "-map".into(), "[out]".into(), "-map".into(), "0:a:0?".into(),
        "-fpsmax".into(), "60".into(), "-pix_fmt".into(), "yuv420p".into(),
    ];
    args.extend(encoder_args(j.nvenc).into_iter().map(String::from));
    args.extend(["-af", "highpass=f=70,loudnorm=I=-14:TP=-1.5:LRA=11", "-c:a", "aac", "-b:a", "160k", "-ar", "48000", "-movflags", "+faststart"].map(String::from));
    args.push(j.out.to_string_lossy().into());
    run_ffmpeg(&j.work, &args, dur, progress).await
}

/// One frame at `at`, laid out like the clip, with the hook in large type.
pub async fn render_cover(video: &str, at: f64, text: &str, layout: &Layout, work: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(work)?;
    std::fs::write(work.join("subs.ass"), still_ass(1080, 1920, text, "Hook", "{\\an5\\fs86}"))?;
    let args: Vec<String> = vec![
        "-ss".into(), format!("{at:.3}"), "-i".into(), video.into(), "-frames:v".into(), "1".into(),
        "-filter_complex".into(), vertical_graph(layout, true), "-map".into(), "[out]".into(), "-q:v".into(), "3".into(), out.to_string_lossy().into(),
    ];
    run_ffmpeg(work, &args, 0.0, |_| {}).await
}

/// Three thumbnail concepts, each a different composition of a strong frame + headline.
pub fn thumbnail_concepts() -> [(&'static str, &'static str, &'static str); 3] {
    [
        // (name, extra video filter, ASS override)
        ("Bold bottom-left headline", "eq=contrast=1.18:saturation=1.35", "{\\an1\\c&H00FFFFFF&\\3c&H00000000&}"),
        ("Punch-in with yellow headline", "crop=iw/1.25:ih/1.25,scale=1280:720,eq=contrast=1.2:saturation=1.4", "{\\an7\\c&H0000E5FF&\\3c&H00000000&\\fs130}"),
        ("Centered statement", "eq=contrast=1.1:brightness=-0.06:saturation=1.25,vignette=PI/4", "{\\an5\\fs140\\c&H00FFFFFF&\\3c&H002020E0&}"),
    ]
}

pub async fn render_thumbnail(video: &str, at: f64, text: &str, concept: usize, work: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(work)?;
    let (_, vf, ov) = thumbnail_concepts()[concept % 3];
    std::fs::write(work.join("subs.ass"), still_ass(1280, 720, text, "Thumb", ov))?;
    let args: Vec<String> = vec![
        "-ss".into(), format!("{at:.3}"), "-i".into(), video.into(), "-frames:v".into(), "1".into(),
        "-vf".into(), format!("scale=1280:720:force_original_aspect_ratio=increase,crop=1280:720,{vf},subtitles=subs.ass"),
        out.to_string_lossy().into(),
    ];
    run_ffmpeg(work, &args, 0.0, |_| {}).await
}

/// Up to four punchy words from a title for thumbnail text.
pub fn headline(title: &str) -> String {
    let stop = ["the", "a", "an", "of", "to", "el", "la", "los", "las", "de", "que", "y", "en", "un", "una"];
    let words: Vec<&str> = title
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '!' && c != '?'))
        .filter(|w| !w.is_empty() && !stop.contains(&w.to_lowercase().as_str()))
        .collect();
    words.iter().take(4).copied().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: f64, t: &str) -> Word {
        Word { s, e: s + 0.3, t: t.into() }
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
        let ass = captions_ass(&words, 100.0, 160.0, Some("no lo vas a creer"));
        assert!(ass.contains("Hook,,0,0,0,,{\\fad(150,250)}NO LO VAS A CREER"));
        assert!(ass.contains("Dialogue: 0,0:00:00.50,0:00:00.90,Cap,,0,0,0,,{\\c&H0000E5FF&\\fscx112\\fscy112}HOLA{\\r} MUNDO"));
        assert!(!ass.contains("ANTES") && !ass.contains("DESPUES"));
    }

    #[test]
    fn caption_events_never_overlap() {
        let words: Vec<Word> = (0..12).map(|i| Word { s: i as f64 * 0.5, e: i as f64 * 0.5 + 0.3, t: "w".into() }).collect();
        let ass = captions_ass(&words, 0.0, 60.0, None);
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
    fn graphs_end_in_out_label() {
        let blur = Layout { kind: "blur".into(), facecam: [0.0; 4] };
        let cam = Layout { kind: "facecam".into(), facecam: [0.7, 0.04, 0.28, 0.3] };
        assert!(vertical_graph(&blur, true).ends_with("subtitles=subs.ass[out]"));
        assert!(vertical_graph(&cam, false).contains("vstack=inputs=2[base]"));
    }

    #[test]
    fn headline_drops_filler_words() {
        assert_eq!(headline("\"No mames, el jefe de la raid!\""), "No mames jefe raid!");
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
        let words: Vec<Word> = (0..120).map(|i| Word { s: 5.0 + i as f64 * 0.5, e: 5.3 + i as f64 * 0.5, t: if i % 7 == 0 { "jajaja".into() } else { "palabra".into() } }).collect();
        for (kind, nvenc) in [("blur", false), ("facecam", crate::media::detect_toolchain().await.nvenc)] {
            let out = dir.path().join(format!("{kind}.mp4"));
            let mut last = 0.0;
            render_clip(
                ClipJob { video: &v, start: 10.0, end: 70.0, words: &words, hook: Some("no lo vas a creer"), layout: Layout { kind: kind.into(), facecam: [0.7, 0.04, 0.28, 0.3] }, subtitles: true, nvenc, work: dir.path().join(kind), out: out.clone() },
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
        render_cover(&v, 20.0, "hook text", &layout, &dir.path().join("c"), &cover).await.unwrap();
        assert_eq!(probe_dims(&cover).await.0, 1080);
        let thumb = dir.path().join("thumb.png");
        render_thumbnail(&v, 20.0, "BIG MOMENT", 1, &dir.path().join("t"), &thumb).await.unwrap();
        assert_eq!(probe_dims(&thumb).await.0, 1280);
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
