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
    /// Speaker number within a clip (0 when speakers haven't been separated).
    #[serde(default)]
    pub sp: u8,
    /// Bleeped in the audio.
    #[serde(default)]
    pub b: bool,
}

// ---------- transcription ----------

/// Writes the audio as raw 16 kHz mono s16le, the form `split_audio` reads.
pub async fn extract_audio(video: &str, raw: &Path) -> Result<()> {
    let out = hidden("ffmpeg")
        .args(["-y", "-v", "error", "-i", video, "-vn", "-ac", "1", "-ar", "16000", "-f", "s16le"])
        .arg(raw)
        .output()
        .await
        .context("couldn't run ffmpeg")?;
    if !out.status.success() {
        return Err(anyhow!("audio extraction failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Audio is cut into pieces of about this length. whisper.cpp picks one language per input
/// and otherwise repeats itself on long files, so each piece gets a fresh start.
const CHUNK_SECS: usize = 180;
/// Each cut lands on the quietest moment in this many seconds before the target.
const CUT_SEARCH_SECS: usize = 6;
const RATE: usize = 16_000;

fn wav_header(samples: usize) -> Vec<u8> {
    let data = (samples * 2) as u32;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&1u16.to_le_bytes()); // mono
    h.extend_from_slice(&(RATE as u32).to_le_bytes());
    h.extend_from_slice(&((RATE * 2) as u32).to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data.to_le_bytes());
    h
}

/// Index of the quietest 50 ms window in `pcm`, so a cut doesn't split a word.
pub fn quietest(pcm: &[i16]) -> usize {
    const WIN: usize = RATE / 20;
    pcm.chunks(WIN)
        .enumerate()
        .min_by_key(|(_, c)| c.iter().map(|s| (*s as i64).abs()).sum::<i64>() / c.len() as i64)
        .map(|(i, _)| i * WIN)
        .unwrap_or(0)
}

/// Splits 16 kHz mono s16le audio into `cNNNN.wav` files in `dir`.
/// Returns each file name and its start time in seconds.
pub fn split_audio(raw: &Path, dir: &Path) -> Result<Vec<(String, f64)>> {
    use std::io::{Read, Write};
    std::fs::create_dir_all(dir)?;
    let mut f = std::io::BufReader::new(std::fs::File::open(raw)?);
    let target = CHUNK_SECS * RATE;
    let mut pending: Vec<i16> = Vec::new();
    let mut out = Vec::new();
    let mut offset = 0usize; // samples already written
    let mut buf = vec![0u8; 2 * RATE * 10];
    let mut eof = false;
    while !eof || !pending.is_empty() {
        while !eof && pending.len() < target {
            let n = f.read(&mut buf)?;
            if n == 0 {
                eof = true;
            }
            pending.extend(buf[..n & !1].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
        }
        let cut = if pending.len() >= target {
            let from = target - CUT_SEARCH_SECS * RATE;
            from + quietest(&pending[from..target])
        } else {
            pending.len()
        };
        let name = format!("c{:04}.wav", out.len());
        let mut w = std::io::BufWriter::new(std::fs::File::create(dir.join(&name))?);
        w.write_all(&wav_header(cut))?;
        for s in &pending[..cut] {
            w.write_all(&s.to_le_bytes())?;
        }
        w.flush()?;
        out.push((name, offset as f64 / RATE as f64));
        offset += cut;
        pending.drain(..cut);
    }
    Ok(out)
}

/// Runs whisper-cli over the pieces in `dir` and joins the results; reports 0..1 progress.
///
/// `-mc 0` stops each segment being fed the previous text and voice detection skips the
/// silence and game noise: both are what send whisper into repeating one line for minutes.
/// The threshold is below the default of 0.5, which dropped half the speech on a stretch of
/// friends talking over game audio. Without `vad` every moment is transcribed: more
/// repeats and worse timings, but it hears some words the other misses.
///
/// With `vad`, the non-speech parts of each piece are silenced rather than cut out. whisper's
/// own `--vad` cuts them out and maps times back, but its timings on the shortened audio are
/// off by up to a second, and a second there can cross a removed 10 s pause: captions showed
/// up 14 s before the words. Silencing keeps every time on the real timeline.
/// The voice detection tool that ships next to the speech engine. Older installs lack it.
pub fn speech_detector(exe: &Path) -> std::path::PathBuf {
    exe.with_file_name("whisper-vad-speech-segments.exe")
}

pub async fn run_whisper(exe: &Path, model: &Path, vad: Option<&Path>, dir: &Path, chunks: &[(String, f64)], mut progress: impl FnMut(f64)) -> Result<Heard> {
    let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).max(2)).unwrap_or(4).to_string();
    let detector = speech_detector(exe);
    // Per piece: the file whisper reads, its start time, and its speech (None: not known).
    let mut inputs: Vec<(String, f64, Option<Vec<(f64, f64)>>)> = Vec::new();
    match vad {
        Some(vad) if detector.is_file() => {
            for (name, start) in chunks {
                // A piece the detector can't read is transcribed as it is, not given up on.
                let Ok(regions) = speech_regions(&detector, vad, &dir.join(name)).await else {
                    inputs.push((name.clone(), *start, None));
                    continue;
                };
                if regions.is_empty() {
                    continue; // nothing said in this piece
                }
                let quiet = format!("m{name}");
                mute_outside(&dir.join(name), &dir.join(&quiet), &regions)?;
                inputs.push((quiet, *start, Some(regions)));
            }
            if inputs.is_empty() {
                return Err(anyhow!("speech engine wrote no transcript"));
            }
        }
        _ => inputs = chunks.iter().map(|(n, s)| (n.clone(), *s, None)).collect(),
    }
    let mut cmd = hidden(&exe.to_string_lossy());
    // Short relative names: hundreds of pieces must fit on one command line.
    cmd.current_dir(dir).arg("-m").arg(model).arg("-f").args(inputs.iter().map(|c| &c.0));
    cmd.args(["-l", "auto", "-mc", "0"]);
    if let (Some(vad), false) = (vad, detector.is_file()) {
        // Older engine without the detector tool: let whisper cut the silence itself.
        cmd.args(["--vad", "-vt", "0.35", "-vm"]).arg(vad);
    }
    let mut child = cmd
        .args(["-ml", "1", "-sow", "-oj", "-pp", "-np", "-t", &threads])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("couldn't start the speech engine")?;
    let mut tail = Vec::new();
    let (mut done, mut last) = (0usize, 0.0f64);
    if let Some(err) = child.stderr.take() {
        let mut lines = BufReader::new(err).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(p) = line.split("progress =").nth(1).and_then(|v| v.trim().trim_end_matches('%').trim().parse::<f64>().ok()) {
                if p < last {
                    done += 1; // progress restarts for each input file
                }
                last = p;
                progress(((done as f64 + p / 100.0) / inputs.len().max(1) as f64).min(1.0));
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
    let mut parts = Vec::new();
    for (name, start, regions) in &inputs {
        // A piece with no speech writes no transcript.
        if let Ok(json) = std::fs::read_to_string(dir.join(format!("{name}.json"))) {
            let (lang, mut words) = parse_whisper_json(&json)?;
            if let Some(r) = regions {
                words = fit_to_speech(words, r);
            }
            parts.push((lang, *start, words));
        }
    }
    if parts.is_empty() {
        return Err(anyhow!("speech engine wrote no transcript"));
    }
    Ok(join_chunks(parts))
}

/// The stretches of speech in one piece, in seconds, from whisper.cpp's voice detector with
/// the same settings `--vad` used.
async fn speech_regions(detector: &Path, vad: &Path, wav: &Path) -> Result<Vec<(f64, f64)>> {
    let out = hidden(&detector.to_string_lossy())
        .arg("-f")
        .arg(wav)
        .arg("-vm")
        .arg(vad)
        .args(["-vt", "0.35", "-np"])
        .output()
        .await
        .context("couldn't start the voice detector")?;
    if !out.status.success() {
        return Err(anyhow!("voice detector failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(parse_speech_regions(&String::from_utf8_lossy(&out.stdout)))
}

/// Reads the detector's lines, `Speech segment 0: start = 106.00, end = 185.00`, which are in
/// hundredths of a second.
pub fn parse_speech_regions(text: &str) -> Vec<(f64, f64)> {
    text.lines()
        .filter_map(|l| {
            let num = |key: &str| l.split(key).nth(1)?.split(',').next()?.trim().parse::<f64>().ok();
            Some((num("start =")? / 100.0, num("end =")? / 100.0))
        })
        .collect()
}

/// Copies a piece written by `split_audio` with everything outside `regions` set to silence.
fn mute_outside(src: &Path, dst: &Path, regions: &[(f64, f64)]) -> Result<()> {
    let bytes = std::fs::read(src)?;
    let pcm = bytes.get(44..).unwrap_or_default();
    let n = pcm.len() / 2;
    let mut out = wav_header(n);
    out.resize(44 + n * 2, 0);
    for &(a, b) in regions {
        let (i, j) = (((a * RATE as f64) as usize).min(n), ((b * RATE as f64) as usize).min(n));
        if i < j {
            out[44 + 2 * i..44 + 2 * j].copy_from_slice(&pcm[2 * i..2 * j]);
        }
    }
    std::fs::write(dst, out)?;
    Ok(())
}

/// Keeps each word inside the stretch of speech it belongs to (the one it overlaps most, or
/// the nearest): it can't start before that speech or run on into the silence after it.
/// Words whisper stacked on one instant are pushed apart (0.12 s each, less if the speech is
/// too short), so captions appear one by one instead of all at once.
pub fn fit_to_speech(mut words: Vec<Word>, regions: &[(f64, f64)]) -> Vec<Word> {
    if regions.is_empty() {
        return words;
    }
    // Overlap with a region, or minus the gap to it when they don't touch.
    let home: Vec<usize> = words
        .iter()
        .map(|w| {
            let fit = |r: &(f64, f64)| w.e.max(w.s).min(r.1) - w.s.max(r.0);
            (0..regions.len()).max_by(|&a, &b| fit(&regions[a]).total_cmp(&fit(&regions[b]))).unwrap_or(0)
        })
        .collect();
    for (r, &(a, b)) in regions.iter().enumerate() {
        let idx: Vec<usize> = (0..words.len()).filter(|&i| home[i] == r).collect();
        for &i in &idx {
            words[i].s = words[i].s.max(a).min(b);
        }
        // A stack shares the time up to the next word.
        let mut k = 0;
        while k < idx.len() {
            let s = words[idx[k]].s;
            let m = (k + 1..idx.len()).find(|&m| words[idx[m]].s != s).unwrap_or(idx.len());
            if m - k > 1 {
                let end = match idx.get(m) {
                    Some(&n) => words[n].s,
                    None => idx[k..m].iter().map(|&i| words[i].e).fold(s, f64::max).min(b),
                };
                let step = (end - s).max(0.0) / (m - k) as f64;
                for (n, &i) in idx[k..m].iter().enumerate() {
                    words[i].s = s + step * n as f64;
                    words[i].e = words[i].s + step;
                }
            }
            k = m;
        }
        let gap = 0.12f64.min((b - a) / idx.len().max(1) as f64);
        let mut prev = f64::NEG_INFINITY;
        for &i in &idx {
            words[i].s = words[i].s.max(prev + gap);
            prev = words[i].s;
        }
        let mut next = b;
        for &i in idx.iter().rev() {
            words[i].s = words[i].s.min(next - gap);
            words[i].e = words[i].e.max(words[i].s + gap).min(next);
            next = words[i].s;
        }
    }
    // Two words that swapped regions would otherwise be out of order.
    words.sort_by(|x, y| x.s.total_cmp(&y.s));
    words
}

/// A joined transcript.
pub struct Heard {
    /// The most common language across pieces with speech.
    pub lang: Option<String>,
    pub words: Vec<Word>,
    /// Time spans of the pieces heard as English, for words that are only slurs in English.
    pub english: Vec<(f64, f64)>,
}

/// Puts pieces back on the recording's timeline, repairs timings and removes repeats.
pub fn join_chunks(parts: Vec<(Option<String>, f64, Vec<Word>)>) -> Heard {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    let mut words = Vec::new();
    let mut english = Vec::new();
    for (lang, start, part) in parts {
        if let (Some(l), Some(last)) = (&lang, part.last()) {
            *counts.entry(l.clone()).or_default() += 1;
            if l == "en" {
                english.push((start, start + last.e + 1.0));
            }
        }
        words.extend(part.into_iter().map(|w| Word { s: w.s + start, e: (w.e + start).max(w.s + start + 0.12), ..w }));
    }
    let lang = counts.into_iter().max_by_key(|(_, n)| *n).map(|(l, _)| l);
    Heard { lang, words: drop_repeats(words), english }
}

/// Removes runs where the same words repeat over and over. The engine's habit of looping on
/// a line survives in short bursts even with the fixes above. Real repetition stays: a word
/// said up to five times, or a phrase said twice.
pub fn drop_repeats(words: Vec<Word>) -> Vec<Word> {
    const LONGEST: usize = 14;
    let key: Vec<String> = words.iter().map(|w| norm(&w.t)).collect();
    let mut keep = vec![true; words.len()];
    let mut i = 0;
    while i < words.len() {
        let mut skipped = false;
        for n in 1..=LONGEST { // shortest first: two copies of a line must not count as one unit
            let mut count = 1;
            while i + (count + 1) * n <= words.len() && key[i..i + n] == key[i + count * n..i + (count + 1) * n] {
                count += 1;
            }
            let (min, kept) = if n == 1 { (6, 3) } else { (3, 2) };
            if count >= min {
                keep[i + kept * n..i + count * n].fill(false);
                i += count * n;
                skipped = true;
                break;
            }
        }
        if !skipped {
            i += 1;
        }
    }
    let mut flags = keep.iter();
    words.into_iter().filter(|_| *flags.next().unwrap()).collect()
}

/// Drops sound descriptions the engine writes as words: `*laughs*`, `*speaking in Spanish*`,
/// `(risas)`. They'd show up in captions, and stars in them looked like censored words.
pub fn drop_sound_tags(words: Vec<Word>) -> Vec<Word> {
    let mut open: Option<char> = None;
    words
        .into_iter()
        .filter(|w| {
            let t = w.t.trim_matches(|c: char| ",.!?¡¿".contains(c));
            if let Some(close) = open {
                if t.ends_with(close) {
                    open = None;
                }
                return false;
            }
            for (start, close) in [('*', '*'), ('(', ')')] {
                if t.starts_with(start) {
                    if !(t.len() > 1 && t.ends_with(close)) {
                        open = Some(close);
                    }
                    return false;
                }
            }
            true
        })
        .collect()
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
            Some(Word { s: seg["offsets"]["from"].as_f64()? / 1000.0, e: seg["offsets"]["to"].as_f64()? / 1000.0, t, sp: 0, b: false })
        })
        .collect();
    Ok((lang, drop_sound_tags(words)))
}

// ---------- speakers ----------

/// Parses the separator's output lines: `0.318 -- 6.865 speaker_00`.
pub fn parse_turns(text: &str) -> Vec<(f64, f64, u32)> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let a = it.next()?.parse().ok()?;
            (it.next()? == "--").then_some(())?;
            let b = it.next()?.parse().ok()?;
            let sp = it.next()?.strip_prefix("speaker_")?.parse().ok()?;
            Some((a, b, sp))
        })
        .collect()
}

/// Separates speakers in a clip's audio. Turn times are relative to the clip.
pub async fn diarize(tool: &crate::tools::SpeakerTool, video: &str, start: f64, dur: f64, work: &Path) -> Result<Vec<(f64, f64, u32)>> {
    let wav = work.join("speakers.wav");
    let out = hidden("ffmpeg")
        .args(["-y", "-v", "error", "-ss", &format!("{start:.3}"), "-i", video, "-t", &format!("{dur:.3}"), "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&wav)
        .output()
        .await?;
    if !out.status.success() {
        return Err(anyhow!("couldn't extract clip audio: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let out = hidden(&tool.exe.to_string_lossy())
        .arg(format!("--segmentation.pyannote-model={}", tool.segmentation.display()))
        .arg(format!("--embedding.model={}", tool.embedding.display()))
        .arg("--clustering.cluster-threshold=0.6")
        .arg(&wav)
        .output()
        .await
        .context("couldn't start the speaker engine")?;
    if !out.status.success() {
        return Err(anyhow!("speaker engine failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(parse_turns(&text))
}

/// Gives each word the speaker it overlaps most, numbered by who talks first. Words outside
/// every turn take the previous word's speaker.
pub fn assign_speakers(words: &mut [Word], turns: &[(f64, f64, u32)], clip_start: f64) {
    let mut order: Vec<u32> = Vec::new();
    let mut last = 0u8;
    for w in words.iter_mut() {
        let (a, b) = (w.s - clip_start, w.e - clip_start);
        let best = turns
            .iter()
            .map(|(s, e, sp)| ((b.min(*e) - a.max(*s)).max(0.0), *sp))
            .filter(|(overlap, _)| *overlap > 0.0)
            .max_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        if let Some((_, sp)) = best {
            let idx = order.iter().position(|o| *o == sp).unwrap_or_else(|| {
                order.push(sp);
                order.len() - 1
            });
            last = idx as u8;
        }
        w.sp = last;
    }
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

// ---------- dead air ----------

/// A stretch this long with nobody talking and little going on is cut from full uploads.
pub const DEAD_MIN: f64 = 45.0;
/// Seconds kept at each edge of a cut, so it doesn't feel abrupt.
const DEAD_KEEP: f64 = 3.0;

/// Stretches to cut from a full stream: no speech for `DEAD_MIN` seconds, not louder than the
/// stream usually is, and few scene changes (AFK, loading screens, queues). Quiet gameplay
/// with action on screen stays.
pub fn dead_spans(sec: &[Second], words: &[Word], duration: f64) -> Vec<(f64, f64)> {
    let mut edges: Vec<(f64, f64)> = Vec::new();
    let mut last_end = 0.0;
    for w in words {
        if w.s - last_end >= DEAD_MIN {
            edges.push((last_end, w.s));
        }
        last_end = last_end.max(w.e);
    }
    if duration - last_end >= DEAD_MIN {
        edges.push((last_end, duration));
    }
    edges
        .into_iter()
        .filter(|(a, b)| {
            let s = slice(sec, *a, *b);
            let n = s.len().max(1) as f64;
            let loud = s.iter().map(|x| x.loud).sum::<f64>() / n;
            let cuts_per_min = s.iter().map(|x| x.scene).sum::<f64>() / n * 60.0;
            loud <= 3.0 && cuts_per_min < 3.0
        })
        .map(|(a, b)| (if a <= 0.0 { 0.0 } else { a + DEAD_KEEP }, if b >= duration { duration } else { b - DEAD_KEEP }))
        .filter(|(a, b)| b - a > 1.0)
        .collect()
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

/// A clip's new words after its stream was transcribed again, with every stretch that was
/// bleeped before still bleeped. A hand bleep the censor doesn't repeat would otherwise be lost.
pub fn keep_bleeps(mut new: Vec<Word>, old: &[Word]) -> Vec<Word> {
    for o in old.iter().filter(|o| o.b) {
        if !new.iter().any(|n| n.b && n.s < o.e && n.e > o.s) {
            new.push(o.clone());
        }
    }
    new.sort_by(|x, y| x.s.total_cmp(&y.s));
    new
}

/// The transcript's words inside a clip, as stored on the clip.
pub fn slice_words(words: &[Word], start: f64, end: f64) -> Vec<Word> {
    words.iter().filter(|w| w.s >= start && w.s < end).cloned().collect()
}

/// Words for a clip after a trim: the clip's own words (with their edits and speakers) where
/// it already had them, and the transcript's for anything newly included.
pub fn retrim(own: &[Word], (old_s, old_e): (f64, f64), transcript: &[Word], start: f64, end: f64) -> Vec<Word> {
    let mut out: Vec<Word> = slice_words(transcript, start, end).into_iter().filter(|w| w.s < old_s || w.s >= old_e).collect();
    out.extend(slice_words(own, start, end));
    out.sort_by(|a, b| a.s.partial_cmp(&b.s).unwrap());
    out
}

/// Text for a clip. Bleeped words are left out entirely: not even a masked `n****` shows.
pub fn words_text(words: &[Word]) -> String {
    words.iter().filter(|w| !w.b).map(|w| w.t.as_str()).collect::<Vec<_>>().join(" ")
}

/// How the app's own editor shows a bleeped word, so it can still be un-bleeped. Never
/// published: captions, transcripts, titles and subtitle tracks leave the word out.
pub const BLEEP_MARK: &str = "[bleep]";

/// A line of the transcript for reading and searching: start, end, text.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Line {
    pub s: f64,
    pub e: f64,
    pub text: String,
}

/// Groups words into readable lines: a new line after a sentence ends, a pause of a second,
/// or 16 words.
pub fn lines(words: &[Word]) -> Vec<Line> {
    lines_of(words, 16)
}

/// `lines` with a chosen word limit. Bleeped words are left out.
pub fn lines_of(words: &[Word], max_words: usize) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut count = 0;
    for w in words.iter().filter(|w| !w.b) {
        let start_new = match out.last() {
            None => true,
            Some(l) => count >= max_words || w.s - l.e > 1.0 || l.text.ends_with(['.', '!', '?']),
        };
        if start_new {
            out.push(Line { s: w.s, e: w.e, text: w.t.clone() });
            count = 1;
        } else if let Some(l) = out.last_mut() {
            l.e = w.e;
            l.text.push(' ');
            l.text.push_str(&w.t);
            count += 1;
        }
    }
    out
}

/// Consecutive runs of one speaker, as index ranges.
fn turns(words: &[Word]) -> Vec<std::ops::Range<usize>> {
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        match out.last_mut() {
            Some(r) if words[r.start].sp == w.sp => r.end = i + 1,
            _ => out.push(i..i + 1),
        }
    }
    out
}

/// Replaces each speaker turn's text. A turn with the same word count keeps its timings and
/// bleeps; one that changed spreads its new words evenly across the turn's time.
///
/// Editing captions never un-bleeps the audio: a bleeped word keeps its own timing even in a
/// changed turn (spread out, the bleep would land on the wrong word), and stays bleeped if its
/// marker is deleted, since the word is still in the audio. The slur typed into a caption is
/// masked and bleeped like a heard one.
pub fn retext(words: &[Word], texts: &[String], censor_words: &[String]) -> Result<Vec<Word>> {
    let spans = turns(words);
    if spans.len() != texts.len() {
        return Err(anyhow!("the clip changed; reopen the editor and try again"));
    }
    let typed = |w: Word| if crate::censor::is_slur(&w.t, censor_words) { Word { t: crate::censor::mask(&w.t), b: true, ..w } } else { w };
    let mut out = Vec::with_capacity(words.len());
    for (span, text) in spans.into_iter().zip(texts) {
        let old = &words[span];
        let new: Vec<&str> = text.split_whitespace().collect();
        if new.len() == old.len() {
            // A bleeped word comes back as its marker: it keeps its original word and bleep.
            out.extend(old.iter().zip(&new).map(|(w, t)| if *t == BLEEP_MARK { Word { b: true, ..w.clone() } } else { typed(Word { t: t.to_string(), ..w.clone() }) }));
        } else {
            let (a, b) = (old[0].s, old[old.len() - 1].e);
            let step = (b - a) / new.len().max(1) as f64;
            let mut bleeped = old.iter().filter(|w| w.b);
            let mut turn: Vec<Word> = new
                .iter()
                .enumerate()
                .map(|(i, t)| match (*t == BLEEP_MARK).then(|| bleeped.next()).flatten() {
                    Some(w) => w.clone(),
                    None => typed(Word { s: a + i as f64 * step, e: a + (i + 1) as f64 * step, t: t.to_string(), sp: old[0].sp, b: *t == BLEEP_MARK || t.contains('*') }),
                })
                .collect();
            turn.extend(bleeped.cloned());
            turn.sort_by(|x, y| x.s.partial_cmp(&y.s).unwrap());
            out.extend(turn);
        }
    }
    Ok(out)
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
    let near: Vec<&str> = words.iter().filter(|w| !w.b && w.s >= peak - 4.0 && w.s <= peak + 4.0).map(|w| w.t.trim()).collect();
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

    fn word(s: f64, t: &str) -> Word {
        Word { s, e: s + 0.3, t: t.into(), sp: 0, b: false }
    }

    fn sentence(start: f64, t: &str) -> Vec<Word> {
        t.split(' ').enumerate().map(|(i, x)| word(start + i as f64 * 0.4, x)).collect()
    }

    fn text(w: &[Word]) -> String {
        w.iter().map(|x| x.t.as_str()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn bleeped_words_never_show_in_text_lines_or_titles() {
        let mut w = sentence(0.0, "my n**** went home");
        w[1].b = true;
        assert_eq!(words_text(&w), "my went home");
        assert_eq!(lines(&w)[0].text, "my went home");
        assert!(!auto_title(&w, 0.5).contains('*'));
    }

    #[test]
    fn the_editor_marker_keeps_a_bleep() {
        let mut w = sentence(0.0, "my n**** went home");
        w[1].b = true;
        let out = retext(&w, &["my [bleep] went home".into()], &[]).unwrap();
        assert!(out[1].b && out[1].t == "n****", "same word count keeps the original");
        let out = retext(&w, &["my [bleep] went home now".into()], &[]).unwrap();
        assert!(out[1].b, "a respread turn still bleeps the marker");
    }

    #[test]
    fn caption_edits_never_move_or_drop_a_bleep() {
        let mut w = sentence(10.0, "yo my n**** went home");
        w[2].b = true;
        let (s, e) = (w[2].s, w[2].e);
        // A changed turn: the bleep keeps the word's own timing instead of being spread out.
        let out = retext(&w, &["hey [bleep] went".into()], &[]).unwrap();
        let b: Vec<&Word> = out.iter().filter(|x| x.b).collect();
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].s, b[0].e), (s, e));
        assert_eq!(words_text(&out), "hey went");
        // The marker deleted: the word is still in the audio, so it stays bleeped and hidden.
        let out = retext(&w, &["yo my went home".into()], &[]).unwrap();
        assert!(out.iter().any(|x| x.b && x.s == s && x.e == e));
        assert_eq!(words_text(&out), "yo my went home");
    }

    #[test]
    fn the_slur_typed_into_a_caption_is_masked_and_bleeped() {
        let w = sentence(0.0, "my friend went home");
        let out = retext(&w, &["my nigga went home".into()], &[]).unwrap();
        assert!(out[1].b && out[1].t == "n****" && out[1].s == w[1].s);
        let out = retext(&w, &["my Niggas went home now".into()], &[]).unwrap();
        assert!(out[1].b && !words_text(&out).to_lowercase().contains("nig"));
    }

    #[test]
    fn transcript_lines_break_at_sentences_and_pauses() {
        let mut w = sentence(0.0, "hey what's up. we good");
        w.extend(sentence(10.0, "later"));
        let l = lines(&w);
        let text: Vec<&str> = l.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(text, ["hey what's up.", "we good", "later"]);
        assert_eq!(l[2].s, 10.0);
    }

    #[test]
    fn long_quiet_silences_are_cut_but_action_and_talk_stay() {
        let mut words = sentence(0.0, "hello there");
        words.extend(sentence(100.0, "back again"));
        words.extend(sentence(200.0, "and again"));
        words.extend(sentence(300.0, "end"));
        let mut sec = vec![Second::default(); 400];
        for s in sec.iter_mut().take(300).skip(110) {
            s.scene = 0.2; // a busy fight: 12 cuts a minute
        }
        let cuts = dead_spans(&sec, &words, 400.0);
        assert_eq!(cuts.len(), 2, "{cuts:?}");
        assert!((cuts[0].0 - 3.7).abs() < 0.01 && (cuts[0].1 - 97.0).abs() < 0.01, "{cuts:?}");
        assert_eq!(cuts[1].1, 400.0, "silence to the end is cut to the end");
    }

    #[test]
    fn sound_tags_are_dropped_but_words_stay() {
        let w = sentence(0.0, "fire *laughs* *speaking in Spanish* Manny! (risas) ok sh*t");
        assert_eq!(text(&drop_sound_tags(w)), "fire Manny! ok sh*t");
    }

    #[test]
    fn trimming_keeps_edits_and_adds_transcript_words() {
        let transcript = vec![word(10.0, "a"), word(20.0, "b"), word(30.0, "c")];
        let own = vec![Word { sp: 2, ..word(20.0, "edited") }];
        let w = retrim(&own, (15.0, 25.0), &transcript, 5.0, 25.0);
        assert_eq!(text(&w), "a edited");
        assert_eq!(w[1].sp, 2);
    }

    #[test]
    fn same_length_edits_keep_timings_and_changed_turns_respread() {
        let mut w = sentence(0.0, "uno dos tres cuatro");
        w[2].sp = 1;
        w[3].sp = 1;
        w[0].b = true;
        let out = retext(&w, &["one two".into(), "three".into()], &[]).unwrap();
        assert_eq!(text(&out), "one two three");
        assert_eq!(out[1].s, w[1].s);
        assert!(out[0].b, "bleep stays on an unchanged word");
        assert_eq!((out[2].s, out[2].e, out[2].sp), (w[2].s, w[3].e, 1));
        assert!(retext(&w, &["only one turn".into()], &[]).is_err());
    }

    #[test]
    fn parses_speaker_turns_and_skips_other_output() {
        let t = "Started\n0.318 -- 6.865 speaker_00\n7.0 -- 9.5 speaker_01\nElapsed seconds: 3.1\n";
        assert_eq!(parse_turns(t), vec![(0.318, 6.865, 0), (7.0, 9.5, 1)]);
    }

    #[test]
    fn words_take_the_speaker_they_overlap_most() {
        let mut w = vec![word(101.0, "a"), word(102.0, "b"), word(103.0, "c"), word(120.0, "d")];
        assign_speakers(&mut w, &[(0.0, 2.5, 7), (2.5, 5.0, 3)], 100.0);
        let sp: Vec<u8> = w.iter().map(|x| x.sp).collect();
        assert_eq!(sp, vec![0, 0, 1, 1], "first voice is 0; a word in no turn keeps the last speaker");
    }

    #[test]
    fn looping_lines_collapse_to_two() {
        let mut w = Vec::new();
        for i in 0..8 {
            w.extend(sentence(i as f64 * 5.0, "I'm gonna put the car here."));
        }
        assert_eq!(text(&drop_repeats(w)), "I'm gonna put the car here. I'm gonna put the car here.");
    }

    #[test]
    fn real_repetition_is_kept() {
        let s = "let's go let's go no no no no wait what";
        assert_eq!(text(&drop_repeats(sentence(0.0, s))), s);
    }

    #[test]
    fn long_single_word_loops_are_cut() {
        let w = sentence(0.0, "yo yo yo yo yo yo yo yo yo okay");
        assert_eq!(text(&drop_repeats(w)), "yo yo yo okay");
    }

    #[test]
    fn pieces_join_on_the_recording_timeline_and_fix_empty_words() {
        let a = (Some("en".to_string()), 0.0, vec![word(1.0, "one")]);
        let b = (Some("es".to_string()), 180.0, vec![Word { s: 2.0, e: 2.0, t: "dos".into(), sp: 0, b: false }]);
        let c = (Some("en".to_string()), 360.0, vec![word(0.5, "three")]);
        let d = (Some("fr".to_string()), 540.0, vec![]);
        let h = join_chunks(vec![a, b, c, d]);
        let (lang, w) = (h.lang, h.words);
        assert_eq!(lang.as_deref(), Some("en"));
        assert_eq!(h.english, vec![(0.0, 2.3), (360.0, 361.8)], "spans of the English pieces only");
        assert_eq!(w[1].s, 182.0);
        assert!(w[1].e > w[1].s);
        assert_eq!(w[2].s, 360.5);
    }

    #[test]
    fn reads_the_voice_detector_output() {
        let t = "Detected 2 speech segments:\nSpeech segment 0: start = 106.00, end = 185.00\nSpeech segment 1: start = 15511.00, end = 15597.00\n";
        assert_eq!(parse_speech_regions(t), vec![(1.06, 1.85), (155.11, 155.97)]);
    }

    #[test]
    fn words_stay_inside_their_speech() {
        let w = |s: f64, e: f64, t: &str| Word { s, e, t: t.into(), sp: 0, b: false };
        // The shape of clip 96: speech at 155.1, 159.8 and 170.2 s with silence between.
        let regions = [(155.11, 155.97), (159.78, 160.13), (170.21, 171.58)];
        let out = fit_to_speech(
            vec![
                w(155.5, 155.9, "up"),
                w(159.9, 170.24, "so"),  // ran across 10 s of silence
                w(170.0, 170.0, "Dude,"), // stacked a little before the speech
                w(170.0, 170.0, "that"),
                w(170.0, 170.5, "guy's"),
                w(170.5, 171.0, "low"),
            ],
            &regions,
        );
        assert_eq!((out[0].s, out[0].e), (155.5, 155.9), "a word inside speech is left alone");
        assert_eq!((out[1].s, out[1].e), (159.9, 160.13), "cut where the speech ends");
        assert_eq!(out[2].s, 170.21, "never before the speech it belongs to");
        assert!(out[2].e > out[2].s && out[3].s >= out[2].e && out[4].s >= out[3].e, "stacked words take turns: {out:?}");
        assert!(out[4].e <= out[5].s && out[5].e == 171.0, "{out:?}");
        for x in &out {
            assert!(regions.iter().any(|r| x.s >= r.0 && x.e <= r.1), "{x:?} is in silence");
        }
    }

    #[test]
    fn muting_keeps_speech_and_length() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("c.wav"), dir.path().join("m.wav"));
        let mut bytes = wav_header(RATE);
        bytes.extend((0..RATE).flat_map(|_| 1000i16.to_le_bytes()));
        std::fs::write(&src, &bytes).unwrap();
        mute_outside(&src, &dst, &[(0.25, 0.5)]).unwrap();
        let out = std::fs::read(&dst).unwrap();
        assert_eq!(out.len(), bytes.len());
        let at = |i: usize| i16::from_le_bytes([out[44 + 2 * i], out[45 + 2 * i]]);
        assert_eq!((at(0), at(RATE / 4), at(RATE / 2 - 1), at(RATE / 2)), (0, 1000, 1000, 0));
    }

    #[test]
    fn cuts_land_in_the_quiet_part() {
        let mut pcm = vec![9000i16; 16000];
        pcm[8000..9000].fill(0);
        let at = quietest(&pcm);
        assert!((8000..9000).contains(&at), "{at}");
    }

    #[test]
    fn audio_splits_into_pieces_that_add_up() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("a.pcm");
        let secs = 400usize;
        let bytes: Vec<u8> = (0..secs * RATE).flat_map(|i| (((i % 200) * 100) as i16).to_le_bytes()).collect();
        std::fs::write(&raw, &bytes).unwrap();
        let chunks = split_audio(&raw, &dir.path().join("c")).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].1, 0.0);
        assert!(chunks[1].1 > 170.0 && chunks[1].1 <= 180.0);
        let total: u64 = chunks.iter().map(|c| std::fs::metadata(dir.path().join("c").join(&c.0)).unwrap().len() - 44).sum();
        assert_eq!(total as usize, secs * RATE * 2, "no audio lost at the cuts");
    }

    fn words_every(sec: f64, until: f64, text: &str) -> Vec<Word> {
        let mut out = vec![];
        let mut t = 0.0;
        while t < until {
            out.push(Word { s: t, e: t + 0.3, t: text.into(), sp: 0, b: false });
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
        assert_eq!(w[1], Word { s: 0.9, e: 1.3, t: "jajaja".into(), sp: 0, b: false });
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
            Word { s: 10.0, e: 10.4, t: "a".into(), sp: 0, b: false },
            Word { s: 12.0, e: 12.4, t: "b".into(), sp: 0, b: false }, // 1.6 s gap before this word
        ];
        assert!((snap_to_pause(13.0, &words, 5.0, false) - 11.75).abs() < 1e-9);
        assert!((snap_to_pause(9.0, &words, 5.0, true) - 10.65).abs() < 1e-9);
        assert_eq!(snap_to_pause(30.0, &words, 5.0, false), 30.0, "nothing in reach");
    }

    #[test]
    fn laughter_and_spanish_hype_are_detected() {
        let words = vec![
            Word { s: 1.0, e: 1.2, t: "jajaja".into(), sp: 0, b: false },
            Word { s: 2.0, e: 2.2, t: "no".into(), sp: 0, b: false },
            Word { s: 2.2, e: 2.4, t: "mames".into(), sp: 0, b: false },
        ];
        let sec = features(5.0, &[], &words, &[]);
        assert_eq!(sec[1].laugh, 1.0);
        assert!(sec[2].hype >= 1.0);
    }

    #[test]
    fn a_marker_typed_over_a_missed_word_bleeps_it() {
        let w = vec![Word { s: 1.0, e: 1.4, t: "my".into(), sp: 0, b: false }, Word { s: 1.4, e: 1.9, t: "word".into(), sp: 0, b: false }];
        let out = retext(&w, &["my [bleep]".into()], &[]).unwrap();
        assert!(out[1].b && out[1].s == 1.4, "bleeped at the word's own time");
        let out = retext(&w, &["my word".into()], &["word".into()]).unwrap();
        assert!(out[1].b, "your own censor words count when typed");
    }

    #[test]
    fn hand_bleeps_survive_a_new_transcript() {
        let w = |s: f64, t: &str, b: bool| Word { s, e: s + 0.4, t: t.into(), sp: 0, b };
        let out = keep_bleeps(vec![w(1.0, "a", false), w(3.0, "c", true)], &[w(2.0, "x", true), w(3.1, "y", true)]);
        assert_eq!(out.iter().map(|x| (x.t.as_str(), x.b)).collect::<Vec<_>>(), vec![("a", false), ("x", true), ("c", true)]);
    }
}
