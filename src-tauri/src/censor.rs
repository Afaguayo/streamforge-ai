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
/// "Nega" and "Nego" are the same mishearing, seen in English speech on real streams:
/// "watch out Nego." and "Nega just got parried!".
const ENGLISH_ONLY: &[&str] = &["negro", "negros", "negroes", "nega", "negas", "nego"];
/// Full spellings the speech engine's own starred forms are checked against (`n***a`).
const STARRED: &[&str] = &["nigga", "niggas", "niggaz", "niggah", "nigger", "niggers"];
/// Unrelated words that share the prefix.
const NOT_SLURS: &[&str] = &["niggle", "niggles", "niggled", "niggling", "niggly", "niggard", "niggardly"];

/// Seconds added around a word, so its edges are covered. Word timings are often off by a
/// couple of tenths, and missing the word costs far more than muting a bit of the next one.
const PAD: f64 = 0.20;
/// A quarter of the engine's words come back with no length at all (stored as 0.12 s), so
/// the real word can sit on either side of its timing; a bleep shorter than this can miss it.
const MIN_LEN: f64 = 0.70;
/// Seconds added on each side of a word only the second listen heard. That listen skips voice
/// detection and its timings are worse: on a real stream it put the word 0.8 s before the
/// first listen's mishearing of the same word.
const SECOND_PASS_SLACK: f64 = 0.30;
/// Some words come back with timings seconds long. A bleep never runs past this, so one bad
/// timing can't silence a whole stretch of speech.
const MAX_LEN: f64 = 1.5;

fn norm(word: &str) -> String {
    word.to_lowercase().replace(['1', '!'], "i").chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>().trim_matches('-').to_string()
}

pub fn is_slur(word: &str, extra: &[String]) -> bool {
    if starred_slur(word) {
        return true;
    }
    // Trailing punctuation goes first, so "niggas!" doesn't turn into "niggasi".
    let w = norm(word.trim_end_matches(|c: char| !c.is_alphanumeric()));
    if w.is_empty() || NOT_SLURS.contains(&w.as_str()) {
        return false;
    }
    w.starts_with(STEM) || VARIANTS.contains(&w.as_str()) || extra.iter().any(|x| norm(x) == w)
}

/// The speech engine sometimes stars the word itself (`n***a,`). Each run of stars stands for
/// one or more letters; the visible letters must fit one of the full spellings.
fn starred_slur(word: &str) -> bool {
    let w: String = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '*').to_lowercase();
    if !w.contains('*') || !w.starts_with('n') || !w.chars().all(|c| c.is_alphanumeric() || c == '*') {
        return false;
    }
    fn fits(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => {
                let rest = &p[p.iter().position(|c| *c != '*').unwrap_or(p.len())..];
                (1..=t.len()).any(|n| fits(rest, &t[n..]))
            }
            Some(c) => t.first() == Some(c) && fits(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = w.chars().collect();
    STARRED.iter().any(|s| fits(&p, &s.chars().collect::<Vec<_>>()))
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

/// Whether `word` is one this censor masked: a letter, then only stars (`n****`), or the
/// speech engine's own starred spelling of the slur (`n***a`), which transcripts made before
/// the censor knew that form still hold. Other words the engine starred (`sh*t`) and sound
/// tags (`*laughs*`) don't count.
pub fn is_masked(word: &str) -> bool {
    let w = word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '*');
    let mut chars = w.chars();
    (chars.next().is_some_and(|c| c.is_alphanumeric()) && w.len() > 1 && chars.all(|c| c == '*')) || starred_slur(word)
}

/// First letter plus stars, keeping any punctuation out: `n****`.
pub fn mask(word: &str) -> String {
    if word.contains('*') && !starred_slur(word) {
        return word.to_string(); // already masked
    }
    // Stars count as letters, so the engine's own `n***a` becomes `n****` like any other.
    let letters: Vec<char> = word.chars().filter(|c| c.is_alphanumeric() || *c == '*').collect();
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
///
/// A hit is skipped only when the middle of it is already bleeped. The word is often said
/// twice within a second or two, so being near a bleep isn't enough.
pub fn add_missed(words: &mut Vec<Word>, second_pass: &[Word], extra: &[String], english: &[(f64, f64)]) -> usize {
    let mut added = 0;
    for hit in second_pass.iter().filter(|w| hit(w, extra, english)) {
        let mid = (hit.s + hit.e) / 2.0;
        let (s, e) = ((hit.s - SECOND_PASS_SLACK).max(0.0), hit.e + SECOND_PASS_SLACK);
        if !words.iter().filter(|w| w.b).map(|w| range(w.s, w.e)).any(|(a, b)| a <= mid && mid <= b) {
            let at = words.partition_point(|w| w.s < s);
            words.insert(at, Word { s, e, t: mask(&hit.t), sp: 0, b: true });
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

/// Moves bleeps from recording time onto the timeline of an upload made of only the `parts`
/// kept (dead air cut). A bleep that starts or ends inside a cut keeps the piece that is in a
/// kept part, and one that spans a cut is bleeped on both sides of the seam.
pub fn onto_parts(ranges: &[(f64, f64)], parts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let mut before = 0.0;
    for (pa, pb) in parts {
        for (a, b) in ranges {
            let (s, e) = (a.max(*pa), b.min(*pb));
            if s < e {
                out.push((before + s - pa, before + e - pa));
            }
        }
        before += pb - pa;
    }
    merge(out)
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
        let second = vec![w(20.1, 20.4, "niggas"), w(25.0, 25.3, "nigga"), w(26.0, 26.3, "fine")];
        assert_eq!(add_missed(&mut words, &second, &[], &[]), 1);
        let shown: Vec<&str> = words.iter().map(|x| x.t.as_str()).collect();
        assert_eq!(shown, ["hello", "n****", "n****", "bye"]);
        assert!(words[2].b && words[2].s < 25.0 && words[2].e > 25.3);
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
    fn engines_own_starred_spelling_is_caught_and_bleeped() {
        // Seen on a real stream: "Yo, it was n***a, I f***ing ran".
        for t in ["n***a,", "N***A", "n*gga", "ni**as.", "\"n***a"] {
            assert!(is_slur(t, &[]), "{t}");
            assert!(is_masked(t), "stored transcripts with this form load as bleeped: {t}");
        }
        for t in ["f***ing", "sh*t", "coj***?", "*laughs*", "nice"] {
            assert!(!is_slur(t, &[]), "{t}");
        }
        let mut words = vec![w(1.0, 1.4, "was"), w(1.4, 2.1, "n***a,"), w(2.1, 2.3, "I")];
        assert_eq!(apply(&mut words, &[], &[]).len(), 1);
        assert_eq!(words[1].t, "n****");
        assert!(words[1].b && is_masked(&words[1].t));
    }

    #[test]
    fn nega_and_nego_mishearings_only_in_english() {
        let mut words = vec![w(5.0, 5.3, "Nega"), w(6.0, 6.3, "Nego."), w(300.0, 300.3, "nega")];
        assert_eq!(apply(&mut words, &[], &[(0.0, 180.0)]).len(), 2);
        assert!(words[0].b && words[1].b && !words[2].b, "Spanish speech keeps the word");
    }

    #[test]
    fn bleeps_reach_past_loose_word_timings() {
        // A word the engine gave no length (stored as 0.12 s) can be off by 0.3 s either way.
        let (a, b) = range(10.0, 10.12);
        assert!(a <= 9.75 && b >= 10.4, "{a}..{b}");
        let (a, b) = range(10.0, 10.5);
        assert!(a <= 9.8 && b >= 10.7, "{a}..{b}");
    }

    #[test]
    fn second_pass_adds_a_repeat_close_to_a_caught_word() {
        // The word said twice, 0.8 s apart; the first listen caught only the first.
        let mut words = vec![w(10.0, 10.12, "nigga"), w(10.8, 10.92, "what")];
        apply(&mut words, &[], &[]);
        let second = vec![w(10.05, 10.17, "nigga"), w(10.8, 10.92, "nigga")];
        assert_eq!(add_missed(&mut words, &second, &[], &[]), 1, "the same spot is not added twice");
        let r = merge(words.iter().filter(|x| x.b).map(|x| range(x.s, x.e)).collect());
        assert!(r.iter().any(|(a, b)| *a <= 10.3 && *b >= 11.4), "second-pass timing is widened: {r:?}");
    }

    #[test]
    fn bleeps_follow_dead_air_cuts() {
        let parts = [(0.0, 100.0), (200.0, 300.0)];
        // Inside a kept part: shifted by the 100 s cut before it.
        assert_eq!(onto_parts(&[(250.0, 251.0)], &parts), vec![(150.0, 151.0)]);
        // Starts inside the cut: the kept piece is still bleeped, not dropped.
        assert_eq!(onto_parts(&[(199.6, 200.5)], &parts), vec![(100.0, 100.5)]);
        // Runs across the cut: bleeped on both sides of the seam.
        assert_eq!(onto_parts(&[(99.5, 200.5)], &parts), vec![(99.5, 100.5)]);
        // Wholly inside the cut: nothing is left to hear.
        assert!(onto_parts(&[(150.0, 151.0)], &parts).is_empty());
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
