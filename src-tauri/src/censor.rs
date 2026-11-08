//! Auto-censor: finds slurs in the transcript so they can be masked in text and bleeped
//! in audio. Transcripts store the masked word; `transcripts.censors` keeps the time ranges.
//!
//! Matching is deliberately narrow. The Spanish word for "black" is ordinary vocabulary on
//! a bilingual stream and must never match. Extra words come from Settings. Speech
//! recognition can still mishear a word, which is why clips can be bleeped by hand as well.

use crate::analysis::Word;

/// Prefix shared by the standard spellings of the word.
const STEM: &str = "nigg";
/// Spellings speech recognition produces that the prefix doesn't cover.
const VARIANTS: &[&str] = &["niga", "nigas", "nigah", "nigaz", "nigguh", "nibba", "nibbas", "n-word", "nword"];
/// Ordinary Spanish ("black"), so only censored where the speech is English: there the speech
/// engine sometimes writes the slur this way. Seen on a real stream: "gash what, Negro?".
const ENGLISH_ONLY: &[&str] = &["negro", "negros", "negroes"];
/// Unrelated words that share the prefix.
const NOT_SLURS: &[&str] = &["niggle", "niggles", "niggled", "niggling", "niggly", "niggard", "niggardly"];

/// Seconds added around a word, so its edges are covered.
const PAD: f64 = 0.10;
/// Word timings are approximate; a bleep shorter than this can miss the word.
const MIN_LEN: f64 = 0.45;
/// Some words come back with timings seconds long. A bleep never runs past this, so one bad
/// timing can't silence a whole stretch of speech.
const MAX_LEN: f64 = 1.5;

fn norm(word: &str) -> String {
    word.to_lowercase().replace(['1', '!'], "i").chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>().trim_matches('-').to_string()
}

pub fn is_slur(word: &str, extra: &[String]) -> bool {
    // Trailing punctuation goes first, so "niggas!" doesn't turn into "niggasi".
    let w = norm(word.trim_end_matches(|c: char| !c.is_alphanumeric()));
    if w.is_empty() || NOT_SLURS.contains(&w.as_str()) {
        return false;
    }
    w.starts_with(STEM) || VARIANTS.contains(&w.as_str()) || extra.iter().any(|x| norm(x) == w)
}

fn english_slur(word: &str) -> bool {
    ENGLISH_ONLY.contains(&norm(word.trim_end_matches(|c: char| !c.is_alphanumeric())).as_str())
}

fn inside(spans: &[(f64, f64)], t: f64) -> bool {
    spans.iter().any(|(a, b)| t >= *a && t <= *b)
}

/// `is_slur`, plus the English-only words when `t` falls in English speech.
fn hit(w: &Word, extra: &[String], english: &[(f64, f64)]) -> bool {
    is_slur(&w.t, extra) || (english_slur(&w.t) && inside(english, w.s))
}

/// Whether `word` is one this censor masked: a letter, then only stars (`n****`). Words the
/// speech engine starred itself (`sh*t`) and sound tags (`*laughs*`) don't count.
pub fn is_masked(word: &str) -> bool {
    let w = word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '*');
    let mut chars = w.chars();
    chars.next().is_some_and(|c| c.is_alphanumeric()) && w.len() > 1 && chars.all(|c| c == '*')
}

/// First letter plus stars, keeping any punctuation out: `n****`.
pub fn mask(word: &str) -> String {
    if word.contains('*') {
        return word.to_string(); // already masked
    }
    let letters: Vec<char> = word.chars().filter(|c| c.is_alphanumeric()).collect();
    let first = letters.first().copied().unwrap_or('*');
    format!("{first}{}", "*".repeat(letters.len().saturating_sub(1).max(1)))
}

/// Masks every match in place and returns the time ranges to silence. `english` holds the
/// spans of English speech.
pub fn apply(words: &mut [Word], extra: &[String], english: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut hits = Vec::new();
    for w in words.iter_mut().filter(|w| hit(w, extra, english)) {
        w.t = mask(&w.t);
        w.b = true;
        hits.push(range(w.s, w.e));
    }
    merge(hits)
}

/// Adds what a second listen caught and the first didn't. No single pass hears everything: on
/// a real stream, each pass found the word in places the other missed. Returns how many.
pub fn add_missed(words: &mut Vec<Word>, second_pass: &[Word], extra: &[String], english: &[(f64, f64)]) -> usize {
    const SAME_SPOT: f64 = 2.0;
    let mut added = 0;
    for hit in second_pass.iter().filter(|w| hit(w, extra, english)) {
        if !words.iter().any(|w| w.b && (w.s - hit.s).abs() <= SAME_SPOT) {
            let at = words.partition_point(|w| w.s < hit.s);
            words.insert(at, Word { s: hit.s, e: hit.e, t: mask(&hit.t), sp: 0, b: true });
            added += 1;
        }
    }
    added
}

/// The window bleeped for a word that runs from `s` to `e`.
pub fn range(s: f64, e: f64) -> (f64, f64) {
    let (a, b) = ((s - PAD).max(0.0), (e + PAD).min(s - PAD + MAX_LEN));
    if b - a >= MIN_LEN {
        return (a, b);
    }
    let grow = (MIN_LEN - (b - a)) / 2.0;
    ((a - grow).max(0.0), b + grow)
}

/// Sorts ranges and joins the ones that touch.
pub fn merge(mut r: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    r.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in r {
        match out.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// FFmpeg's sine source starts at 1/8 volume (about -21 dB); 2.2x brings the beep to about
/// -15 dB, near speech level. At 0.2x it measured -35 dB in a real clip: a dropout, not a beep.
/// FFmpeg audio graph ending in `[aout]`: silences `ranges` and, unless `style` is "mute",
/// puts a tone over them. `pre` is any filter chain to run first, ending in a comma.
/// `dur` is the length of the audio being processed.
pub fn audio_graph(input: &str, pre: &str, ranges: &[(f64, f64)], style: &str, dur: f64) -> String {
    let on = ranges.iter().map(|(a, b)| format!("between(t,{a:.3},{b:.3})")).collect::<Vec<_>>().join("+");
    let silenced = format!("{input}{pre}volume=0:enable='{on}'");
    if style == "mute" {
        return format!("{silenced}[aout]");
    }
    format!(
        "{silenced}[voice];sine=frequency=1000:sample_rate=48000:duration={dur:.3},volume=0:enable='not({on})',volume=2.2[tone];\
[voice][tone]amix=inputs=2:normalize=0:duration=first[aout]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: f64, e: f64, t: &str) -> Word {
        Word { s, e, t: t.into(), sp: 0, b: false }
    }

    #[test]
    fn matches_spellings_with_punctuation_and_case() {
        for t in ["nigga", "Nigga,", "NIGGAS!", "niggers.", "n1gga", "niga", "n-word"] {
            assert!(is_slur(t, &[]), "{t}");
        }
    }

    #[test]
    fn leaves_ordinary_words_alone() {
        for t in ["negro", "negra", "negros", "Niger", "Nigeria", "night", "niggle", "nice", "ni"] {
            assert!(!is_slur(t, &[]), "{t}");
        }
    }

    #[test]
    fn custom_words_match_whole_words_only() {
        let extra = vec!["Badword".to_string()];
        assert!(is_slur("badword!", &extra));
        assert!(!is_slur("badwords", &extra));
    }

    #[test]
    fn masks_text_and_returns_padded_ranges() {
        let mut words = vec![w(10.0, 10.3, "hello"), w(10.4, 10.7, "nigga,"), w(11.0, 11.2, "ok")];
        let r = apply(&mut words, &[], &[]);
        assert_eq!(words[1].t, "n****");
        assert!(words[1].b && !words[0].b);
        assert_eq!(words[0].t, "hello");
        assert_eq!(r.len(), 1);
        assert!(r[0].0 <= 10.4 && r[0].1 >= 10.7);
    }

    #[test]
    fn second_pass_only_adds_new_spots() {
        let mut words = vec![w(10.0, 10.3, "hello"), w(20.0, 20.3, "nigga"), w(30.0, 30.3, "bye")];
        apply(&mut words, &[], &[]);
        let second = vec![w(20.5, 20.8, "niggas"), w(25.0, 25.3, "nigga"), w(26.0, 26.3, "fine")];
        assert_eq!(add_missed(&mut words, &second, &[], &[]), 1);
        let shown: Vec<&str> = words.iter().map(|x| x.t.as_str()).collect();
        assert_eq!(shown, ["hello", "n****", "n****", "bye"]);
        assert!(words[2].b && words[2].s == 25.0);
    }

    #[test]
    fn spanish_word_is_censored_only_in_english_speech() {
        let mut words = vec![w(5.0, 5.3, "Negro?"), w(200.0, 200.3, "negro."), w(201.0, 201.3, "Nigeria")];
        let r = apply(&mut words, &[], &[(0.0, 180.0)]);
        assert_eq!(r.len(), 1);
        assert_eq!(words[0].t, "N****");
        assert_eq!(words[1].t, "negro.", "Spanish speech keeps the word");
        assert_eq!(words[2].t, "Nigeria");
    }

    #[test]
    fn very_short_words_still_get_a_full_bleep() {
        let (a, b) = range(5.0, 5.05);
        assert!((b - a - MIN_LEN).abs() < 1e-9);
        assert!(a < 5.0 && b > 5.05);
    }

    #[test]
    fn only_this_censors_masks_count_as_bleeps() {
        for t in ["n****", "N****?", "n*****,"] {
            assert!(is_masked(t), "{t}");
        }
        for t in ["*laughs*", "*speaking", "Spanish*", "sh*t", "p*cky.", "*", "word"] {
            assert!(!is_masked(t), "{t}");
        }
    }

    #[test]
    fn badly_timed_words_get_a_short_bleep() {
        let (a, b) = range(17.1, 24.3);
        assert!(b - a <= MAX_LEN + 1e-9, "{a}..{b}");
        assert!(a < 17.1);
    }

    #[test]
    fn range_never_starts_before_zero() {
        assert_eq!(range(0.0, 1.0).0, 0.0);
    }

    #[test]
    fn close_ranges_merge() {
        assert_eq!(merge(vec![(5.0, 6.0), (1.0, 2.0), (1.5, 3.0)]), vec![(1.0, 3.0), (5.0, 6.0)]);
    }

    #[test]
    fn graph_silences_and_adds_tone() {
        let g = audio_graph("[0:a:0]", "", &[(1.0, 2.0), (3.0, 4.0)], "beep", 60.0);
        assert!(g.contains("between(t,1.000,2.000)+between(t,3.000,4.000)"));
        assert!(g.contains("sine=frequency=1000") && g.ends_with("[aout]"));
        let m = audio_graph("[0:a:0]", "", &[(1.0, 2.0)], "mute", 60.0);
        assert!(!m.contains("sine") && m.ends_with("[aout]"));
    }
}
