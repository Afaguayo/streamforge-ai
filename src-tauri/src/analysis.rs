//! Transcription (whisper.cpp) and moment detection.
//!
//! Signals per second of the recording:
//! - loudness (FFmpeg astats RMS), relative to the stream's own median, so a loud game
//!   isn't "always exciting";
//! - speech density and laughter/excitement words from the transcript (ES + EN);
//! - scene changes (FFmpeg scene score).
//! Peaks become candidate windows of 60–120 s whose edges are moved to pauses in speech,
//! so clips start and end on full sentences. Each candidate gets component scores; when a
//! Claude API key is set, `llm.rs` re-scores them from the transcript.

use crate::media::hidden;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

pub const MIN_CLIP: f64 = 60.0;
pub const MAX_CLIP: f64 = 120.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub s: f64,
    pub e: f64,
    pub t: String,
}

// ---------- transcription ----------

pub async fn extract_audio(video: &str, wav: &Path) -> Result<()> {
    let out = hidden("ffmpeg")
        .args(["-y", "-v", "error", "-i", video, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(wav)
        .output()
        .await
        .context("couldn't run ffmpeg")?;
    if !out.status.success() {
        return Err(anyhow!("audio extraction failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Runs whisper-cli with one word per segment; reports 0..1 progress.
pub async fn run_whisper(exe: &Path, model: &Path, wav: &Path, out_base: &Path, mut progress: impl FnMut(f64)) -> Result<(Option<String>, Vec<Word>)> {
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).max(2)).unwrap_or(4).to_string();
    let mut child = hidden(&exe.to_string_lossy())
        .arg("-m").arg(model)
        .arg("-f").arg(wav)
        .args(["-l", "auto", "-ml", "1", "-sow", "-oj", "-pp", "-np", "-t", &threads])
        .arg("-of").arg(out_base)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("couldn't start the speech engine")?;
    let mut tail = Vec::new();
    if let Some(err) = child.stderr.take() {
        let mut lines = BufReader::new(err).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(p) = line.split("progress =").nth(1).and_then(|v| v.trim().trim_end_matches('%').trim().parse::<f64>().ok()) {
                progress(p / 100.0);
            }
            tail.push(line);
            if tail.len() > 20 {
                tail.remove(0);
            }
        }
    }
    let status = child.wait().await?;
    if !status.success() {
        return Err(anyhow!("speech engine failed: {}", tail.join(" | ")));
    }
    let json = std::fs::read_to_string(out_base.with_extension("json")).context("speech engine wrote no transcript")?;
    parse_whisper_json(&json)
}

pub fn parse_whisper_json(json: &str) -> Result<(Option<String>, Vec<Word>)> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let lang = v["result"]["language"].as_str().map(String::from);
    let words = v["transcription"]
        .as_array()
        .ok_or_else(|| anyhow!("transcript has no segments"))?
        .iter()
        .filter_map(|seg| {
            let t = seg["text"].as_str()?.trim().to_string();
            if t.is_empty() || t.starts_with('[') {
                return None; // skips [MUSIC], [BLANK_AUDIO] markers
            }
            Some(Word { s: seg["offsets"]["from"].as_f64()? / 1000.0, e: seg["offsets"]["to"].as_f64()? / 1000.0, t })
        })
        .collect();
    Ok((lang, words))
}

// ---------- audio + scene signals ----------

/// Loudness (RMS dBFS) for each second.
pub async fn loudness_per_second(video: &str) -> Result<Vec<f64>> {
    let out = hidden("ffmpeg")
        .args(["-v", "error", "-i", video, "-vn", "-af"])
        .arg("aresample=8000,asetnsamples=n=8000:p=0,astats=metadata=1:reset=1,ametadata=print:key=lavfi.astats.Overall.RMS_level:file=-")
        .args(["-f", "null", "-"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(anyhow!("loudness analysis failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(parse_rms(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse_rms(text: &str) -> Vec<f64> {
    text.lines()
        .filter_map(|l| l.split("RMS_level=").nth(1))
        .map(|v| v.trim().parse::<f64>().ok().filter(|x| x.is_finite()).unwrap_or(-90.0))
        .collect()
}

/// Timestamps (s) of hard scene changes, sampled at 2 fps on a small frame for speed.
pub async fn scene_changes(video: &str) -> Result<Vec<f64>> {
    let out = hidden("ffmpeg")
        .args(["-v", "info", "-i", video, "-an", "-vf", "fps=2,scale=160:-2,select='gt(scene,0.35)',showinfo", "-f", "null", "-"])
        .output()
        .await?;
    let err = String::from_utf8_lossy(&out.stderr);
    Ok(err
        .lines()
        .filter(|l| l.contains("Parsed_showinfo"))
        .filter_map(|l| l.split("pts_time:").nth(1))
        .filter_map(|v| v.split_whitespace().next()?.parse().ok())
        .collect())
}

// ---------- features ----------

const LAUGH: &[&str] = &["jaja", "jajaja", "jeje", "haha", "hahaha", "lol", "lmao", "xd", "jsjs"];
const HYPE: &[&str] = &[
    "no mames", "no manches", "wey", "güey", "qué", "verga", "chingado", "chale", "órale", "dios", "increíble", "mira", "vamos", "noo",
    "what", "no way", "oh my god", "omg", "bro", "dude", "wtf", "insane", "let's go", "lets go", "holy", "wait",
];

#[derive(Debug, Clone, Default)]
pub struct Second {
    pub loud: f64,   // dB above the stream's median
    pub words: f64,  // words spoken
    pub laugh: f64,  // laughter tokens
    pub hype: f64,   // excitement phrases
    pub scene: f64,  // scene cuts
}

fn norm(t: &str) -> String {
    t.to_lowercase().chars().filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '\'').collect()
}

pub fn features(duration: f64, rms: &[f64], words: &[Word], scenes: &[f64]) -> Vec<Second> {
    let n = duration.ceil().max(1.0) as usize;
    let mut sec = vec![Second::default(); n];
    let mut sorted: Vec<f64> = rms.iter().copied().filter(|v| *v > -89.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(-30.0);
    for (i, s) in sec.iter_mut().enumerate() {
        s.loud = rms.get(i).map(|v| (v - median).max(-20.0)).unwrap_or(0.0);
    }
    for (i, w) in words.iter().enumerate() {
        let idx = (w.s as usize).min(n - 1);
        let t = norm(&w.t);
        sec[idx].words += 1.0;
        if LAUGH.iter().any(|l| t.starts_with(l)) || t.contains("jaja") || t.contains("haha") {
            sec[idx].laugh += 1.0;
        }
        // Two-word phrases ("no mames", "oh my god") are matched against this word plus the next two.
        let phrase = words[i..words.len().min(i + 3)].iter().map(|w| norm(&w.t)).collect::<Vec<_>>().join(" ");
        if HYPE.iter().any(|h| phrase.starts_with(h)) {
            sec[idx].hype += 1.0;
        }
        if w.t.contains('!') || w.t.contains('?') {
            sec[idx].hype += 0.3;
        }
    }
    for &t in scenes {
        if let Some(s) = sec.get_mut(t as usize) {
            s.scene += 1.0;
        }
    }
    sec
}

/// Excitement per second, smoothed over ±3 s. Weights come from `weights` (learning loop).
pub fn excitement(sec: &[Second], w: &Weights) -> Vec<f64> {
    let raw: Vec<f64> = sec
        .iter()
        .map(|s| w.loud * (s.loud / 6.0).clamp(-1.0, 3.0) + w.speech * (s.words / 3.0).min(2.0) + w.laugh * s.laugh.min(3.0) + w.hype * s.hype.min(3.0) + w.scene * s.scene.min(1.0))
        .collect();
    (0..raw.len())
        .map(|i| {
            let lo = i.saturating_sub(3);
            let hi = (i + 4).min(raw.len());
            raw[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Weights {
    pub loud: f64,
    pub speech: f64,
    pub laugh: f64,
    pub hype: f64,
    pub scene: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights { loud: 1.0, speech: 0.35, laugh: 1.2, hype: 0.8, scene: 0.25 }
    }
}

// ---------- candidates ----------

/// Moves `t` to the nearest pause (≥0.6 s without speech) within `reach` seconds, searching
/// backwards for starts and forwards for ends.
pub fn snap_to_pause(t: f64, words: &[Word], reach: f64, forward: bool) -> f64 {
    let gaps: Vec<f64> = words
        .windows(2)
        .filter(|p| p[1].s - p[0].e >= 0.6)
        .map(|p| if forward { p[0].e + 0.25 } else { p[1].s - 0.25 })
        .collect();
    let best = gaps
        .iter()
        .copied()
        .filter(|g| if forward { *g >= t && *g - t <= reach } else { *g <= t && t - *g <= reach })
        .min_by(|a, b| (a - t).abs().partial_cmp(&(b - t).abs()).unwrap());
    best.unwrap_or(t)
}

#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub start: f64,
    pub end: f64,
    pub peak: f64,
    pub scores: Scores,
    pub category: String,
    pub transcript: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scores {
    pub hook: f64,
    pub entertainment: f64,
    pub emotion: f64,
    pub context: f64,
    pub replay: f64,
    pub shareability: f64,
    pub trend: f64,
    pub total: f64,
}

fn text_between(words: &[Word], a: f64, b: f64) -> String {
    words.iter().filter(|w| w.s >= a && w.e <= b).map(|w| w.t.as_str()).collect::<Vec<_>>().join(" ")
}

/// Finds up to `n` non-overlapping windows, each at least MIN_CLIP and at most MAX_CLIP long.
/// A recording shorter than MIN_CLIP yields none: clips are never padded to reach 60 s.
pub fn candidates(sec: &[Second], exc: &[f64], words: &[Word], duration: f64, n: usize) -> Vec<Candidate> {
    if duration < MIN_CLIP {
        return vec![];
    }
    let mut order: Vec<usize> = (0..exc.len()).collect();
    order.sort_by(|a, b| exc[*b].partial_cmp(&exc[*a]).unwrap());
    let mut taken: Vec<(f64, f64)> = Vec::new();
    let mut out = Vec::new();
    for peak in order {
        if out.len() >= n {
            break;
        }
        let p = peak as f64;
        if taken.iter().any(|(a, b)| p >= *a - 15.0 && p <= *b + 15.0) {
            continue;
        }
        // Lead-in for context (~35 s before the peak), payoff after it (~40 s).
        let mut start = (p - 35.0).max(0.0);
        let mut end = (p + 40.0).min(duration);
        if end - start < MIN_CLIP {
            if start == 0.0 { end = MIN_CLIP.min(duration) } else { start = (end - MIN_CLIP).max(0.0) }
        }
        start = snap_to_pause(start, words, 8.0, false).max(0.0);
        end = snap_to_pause(end, words, 8.0, true).min(duration);
        if end - start < MIN_CLIP {
            end = (start + MIN_CLIP).min(duration);
            start = (end - MIN_CLIP).max(0.0);
        }
        if end - start > MAX_CLIP {
            end = start + MAX_CLIP;
        }
        if end - start < MIN_CLIP - 0.01 || taken.iter().any(|(a, b)| start < *b && end > *a) {
            continue;
        }
        taken.push((start, end));
        let scores = score_window(sec, exc, start, end, words);
        let category = categorize(sec, start, end);
        let transcript = text_between(words, start, end);
        let title = auto_title(words, p);
        out.push(Candidate { start, end, peak: p, scores, category, transcript, title });
    }
    out.sort_by(|a, b| b.scores.total.partial_cmp(&a.scores.total).unwrap());
    out
}

fn slice(sec: &[Second], a: f64, b: f64) -> &[Second] {
    let a = (a as usize).min(sec.len());
    let b = (b.ceil() as usize).min(sec.len()).max(a);
    &sec[a..b]
}

fn pct(x: f64) -> f64 {
    (x * 100.0).clamp(0.0, 100.0).round()
}

pub fn score_window(sec: &[Second], exc: &[f64], a: f64, b: f64, words: &[Word]) -> Scores {
    let s = slice(sec, a, b);
    let e = &exc[(a as usize).min(exc.len())..(b.ceil() as usize).min(exc.len()).max((a as usize).min(exc.len()))];
    let len = s.len().max(1) as f64;
    let mean = |f: &dyn Fn(&Second) -> f64| s.iter().map(f).sum::<f64>() / len;
    let peak = e.iter().cloned().fold(0.0, f64::max);
    let first5 = e.iter().take(5).sum::<f64>() / 5.0;
    let speech = mean(&|x| x.words);
    let laughs = s.iter().map(|x| x.laugh).sum::<f64>();
    let hype = s.iter().map(|x| x.hype).sum::<f64>();
    // Context: starts and ends on a pause, and enough talking to follow the story.
    let starts_clean = words.windows(2).any(|p| p[1].s - p[0].e >= 0.6 && (p[1].s - 0.25 - a).abs() < 0.3);
    let ends_clean = words.windows(2).any(|p| p[1].s - p[0].e >= 0.6 && (p[0].e + 0.25 - b).abs() < 0.3);
    let mut sc = Scores {
        hook: pct(first5 / 2.5),
        entertainment: pct((laughs * 0.15 + hype * 0.08 + speech * 0.12).min(1.0)),
        emotion: pct(peak / 3.0),
        context: pct(0.3 + 0.25 * starts_clean as u8 as f64 + 0.25 * ends_clean as u8 as f64 + (speech / 3.0).min(0.2)),
        replay: pct((peak / 3.0) * 0.6 + (laughs.min(5.0) / 5.0) * 0.4),
        shareability: pct((laughs.min(6.0) / 6.0) * 0.5 + (hype.min(10.0) / 10.0) * 0.5),
        trend: 0.0, // only set from trend research / Claude
        total: 0.0,
    };
    sc.total = (sc.hook * 0.22 + sc.entertainment * 0.22 + sc.emotion * 0.16 + sc.context * 0.16 + sc.replay * 0.12 + sc.shareability * 0.12).round();
    sc
}

pub fn categorize(sec: &[Second], a: f64, b: f64) -> String {
    let s = slice(sec, a, b);
    let laughs: f64 = s.iter().map(|x| x.laugh).sum();
    let hype: f64 = s.iter().map(|x| x.hype).sum();
    let loud = s.iter().map(|x| x.loud).fold(f64::MIN, f64::max);
    let talk = s.iter().map(|x| x.words).sum::<f64>() / s.len().max(1) as f64;
    if laughs >= 3.0 { "funny" } else if loud >= 9.0 && hype >= 3.0 { "reaction" } else if talk >= 2.2 { "conversation" } else if loud >= 6.0 { "gameplay" } else { "moment" }.to_string()
}

/// Title from the words around the peak, used when no Claude key is set.
pub fn auto_title(words: &[Word], peak: f64) -> String {
    let near: Vec<&str> = words.iter().filter(|w| w.s >= peak - 4.0 && w.s <= peak + 4.0).map(|w| w.t.trim()).collect();
    let mut t = near.iter().take(9).copied().collect::<Vec<_>>().join(" ");
    t = t.trim_matches(|c: char| c == ',' || c == '.' || c == ' ').to_string();
    if t.is_empty() { "Stream highlight".into() } else { format!("\"{}\"", capitalize(&t)) }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words_every(sec: f64, until: f64, text: &str) -> Vec<Word> {
        let mut out = vec![];
        let mut t = 0.0;
        while t < until {
            out.push(Word { s: t, e: t + 0.3, t: text.into() });
            t += sec;
        }
        out
    }

    #[test]
    fn parses_whisper_word_json() {
        let j = r#"{"result":{"language":"es"},"transcription":[
            {"offsets":{"from":0,"to":420},"text":" Hola"},
            {"offsets":{"from":420,"to":900},"text":" [MUSIC]"},
            {"offsets":{"from":900,"to":1300},"text":" jajaja"}]}"#;
        let (lang, w) = parse_whisper_json(j).unwrap();
        assert_eq!(lang.as_deref(), Some("es"));
        assert_eq!(w.len(), 2);
        assert_eq!(w[1], Word { s: 0.9, e: 1.3, t: "jajaja".into() });
    }

    #[test]
    fn parses_rms_lines() {
        let t = "frame:0 pts:0 pts_time:0\nlavfi.astats.Overall.RMS_level=-23.5\nframe:1\nlavfi.astats.Overall.RMS_level=-inf\n";
        assert_eq!(parse_rms(t), vec![-23.5, -90.0]);
    }

    #[test]
    fn short_recordings_give_no_clips_instead_of_padding() {
        let sec = vec![Second::default(); 40];
        let exc = vec![1.0; 40];
        assert!(candidates(&sec, &exc, &[], 40.0, 5).is_empty());
    }

    #[test]
    fn candidates_are_60_to_120s_and_do_not_overlap() {
        let dur = 900.0;
        let words = words_every(0.5, dur, "hola");
        let mut rms = vec![-30.0; 900];
        for i in 300..310 { rms[i] = -10.0; }
        for i in 700..706 { rms[i] = -12.0; }
        let sec = features(dur, &rms, &words, &[]);
        let exc = excitement(&sec, &Weights::default());
        let c = candidates(&sec, &exc, &words, dur, 4);
        assert!(!c.is_empty());
        for x in &c {
            let len = x.end - x.start;
            assert!((MIN_CLIP - 0.01..=MAX_CLIP + 0.01).contains(&len), "len {len}");
        }
        for i in 0..c.len() {
            for j in i + 1..c.len() {
                assert!(c[i].end <= c[j].start || c[j].end <= c[i].start);
            }
        }
        assert!(c.iter().any(|x| x.start <= 305.0 && x.end >= 305.0), "loudest moment is covered");
    }

    #[test]
    fn edges_snap_to_pauses() {
        let words = vec![
            Word { s: 10.0, e: 10.4, t: "a".into() },
            Word { s: 12.0, e: 12.4, t: "b".into() }, // 1.6 s gap before this word
        ];
        assert!((snap_to_pause(13.0, &words, 5.0, false) - 11.75).abs() < 1e-9);
        assert!((snap_to_pause(9.0, &words, 5.0, true) - 10.65).abs() < 1e-9);
        assert_eq!(snap_to_pause(30.0, &words, 5.0, false), 30.0, "nothing in reach");
    }

    #[test]
    fn laughter_and_spanish_hype_are_detected() {
        let words = vec![
            Word { s: 1.0, e: 1.2, t: "jajaja".into() },
            Word { s: 2.0, e: 2.2, t: "no".into() },
            Word { s: 2.2, e: 2.4, t: "mames".into() },
        ];
        let sec = features(5.0, &[], &words, &[]);
        assert_eq!(sec[1].laugh, 1.0);
        assert!(sec[2].hype >= 1.0);
    }
}
