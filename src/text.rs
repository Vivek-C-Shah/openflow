//! Text utilities: rule-based cleanup (the always-available fallback), the
//! guard that rejects AI output that drifted from what was said, Whisper
//! hallucination filtering, and snippet expansion.

use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Default,
    Casual,
    Email,
    Code,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" | "prose" => Some(Style::Default),
            "casual" | "chat" => Some(Style::Casual),
            "email" | "formal" => Some(Style::Email),
            "code" | "terminal" => Some(Style::Code),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Style::Default => "default",
            Style::Casual => "casual",
            Style::Email => "email",
            Style::Code => "code",
        }
    }
}

const FILLERS: &[&str] = &[
    "um", "umm", "ummm", "uh", "uhh", "uhhh", "uhm", "uhmm", "erm", "er", "err", "hmm", "hmmm", "mm",
    "mmm",
];

const AI_HINTS: &[&str] = &[
    "no wait", "wait no", "actually", "i mean", "scratch that", "sorry", "no no", "rather", "like",
    "you know", "basically", "nahi nahi", "matlab", "new line", "new paragraph", "comma", "period",
    "full stop", "question mark",
];

fn core(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase()
}

/// Lowercased words, apostrophes kept ("don't" is one word).
pub fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\'').to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Is AI cleanup worth its latency? Very short, clean utterances ("yes",
/// "sounds good") are handled instantly by the rules.
pub fn needs_ai(raw: &str) -> bool {
    let w = words(raw);
    if w.len() > 3 {
        return true;
    }
    let lower = format!(" {} ", w.join(" "));
    w.iter().any(|x| FILLERS.contains(&x.as_str()))
        || AI_HINTS.iter().any(|h| lower.contains(&format!(" {h} ")))
}

/// Rule-based cleanup: never fails, never invents words.
pub fn rules(raw: &str, style: Style) -> String {
    let mut out: Vec<String> = Vec::new();
    for tok in raw.split_whitespace() {
        let c = core(tok);
        if FILLERS.contains(&c.as_str()) {
            // keep sentence-ending punctuation the filler carried
            if let Some(p) = tok.chars().last().filter(|ch| ".?!".contains(*ch)) {
                if let Some(last) = out.last_mut() {
                    if !last.ends_with(['.', '?', '!']) {
                        while last.ends_with([',', ';', ':']) {
                            last.pop();
                        }
                        last.push(p);
                    }
                }
            }
            continue;
        }
        if let Some(prev) = out.last() {
            if !c.is_empty() && core(prev) == c && !prev.ends_with(['.', '?', '!', ',']) {
                *out.last_mut().unwrap() = tok.to_string(); // "I I think" -> "I think"
                continue;
            }
        }
        out.push(tok.to_string());
    }

    let mut s = out.join(" ");
    s = s.replace(" ,", ",").replace(",,", ",").replace(" .", ".").replace(" ?", "?");
    let s = s.trim_start_matches(|c: char| c == ',' || c == '.' || c.is_whitespace()).trim_end_matches(',');
    let mut s = s.trim().to_string();
    if s.is_empty() {
        return s;
    }

    // standalone "i" -> "I"
    s = s
        .split(' ')
        .map(|t| {
            let c = core(t);
            if c == "i" || c.starts_with("i'") {
                t.replacen('i', "I", 1)
            } else {
                t.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    if style != Style::Code {
        s = capitalize_sentences(&s);
        let n = words(&s).len();
        let ends_alnum = s.chars().last().map(|c| c.is_alphanumeric()).unwrap_or(false);
        let wants_period = match style {
            Style::Casual => n >= 8,
            _ => n >= 3,
        };
        if wants_period && ends_alnum {
            s.push('.');
        }
    }
    s
}

fn capitalize_sentences(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cap = true;
    for ch in s.chars() {
        if cap && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            cap = false;
        } else {
            out.push(ch);
            if ".?!\n".contains(ch) {
                cap = true;
            } else if !ch.is_whitespace() && !"\"'(".contains(ch) {
                cap = false;
            }
        }
    }
    out
}

fn is_mostly_non_latin(s: &str) -> bool {
    let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return false;
    }
    let ascii = letters.iter().filter(|c| c.is_ascii()).count();
    ascii * 2 < letters.len()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The mistake-proofing guard: accept AI output only if it is still what the
/// speaker said. Rejects answers, additions, and large deletions.
pub fn accept(raw: &str, out: &str, dictionary: &[String]) -> bool {
    let out = out.trim();
    if out.is_empty() {
        return false;
    }
    let rw = words(raw);
    let ow = words(out);
    if rw.is_empty() || ow.is_empty() {
        return false;
    }
    // Added content (e.g. answered a question, wrote the poem it was asked for).
    if ow.len() > rw.len() * 13 / 10 + 4 {
        return false;
    }
    // Dropped most of it. ("scratch that" can delete a lot, so be lenient.)
    if rw.len() >= 6 && ow.len() * 4 < rw.len() {
        return false;
    }
    // Transliteration (Devanagari -> Latin) changes every word; length checks only.
    if is_mostly_non_latin(raw) != is_mostly_non_latin(out) {
        return true;
    }
    let mut known: HashSet<String> = rw.iter().cloned().collect();
    for d in dictionary {
        known.extend(words(d));
    }
    let novel = ow
        .iter()
        .filter(|w| {
            if known.contains(*w) || w.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            // spelling / grammar fixes: close to a word that was said
            let max = if w.chars().count() >= 6 { 2 } else { 1 };
            !(w.chars().count() >= 3 && known.iter().any(|k| levenshtein(w, k) <= max))
        })
        .count();
    let budget = ((ow.len() as f32) * 0.25).max(2.0);
    (novel as f32) <= budget
}

/// Strip wrappers small models like to add.
pub fn sanitize_llm(out: &str) -> String {
    let mut s = out.trim().to_string();
    if s.starts_with("```") {
        s = s.lines().skip(1).collect::<Vec<_>>().join("\n");
        if let Some(i) = s.rfind("```") {
            s.truncate(i);
        }
    }
    s = s.replace("<transcript>", "").replace("</transcript>", "");
    s = s.replace("<cleaned>", "").replace("</cleaned>", "");
    let s2 = s.trim().to_string();
    let mut lines: Vec<&str> = s2.lines().collect();
    if lines.len() > 1 {
        let first = lines[0].trim().to_lowercase();
        if first.ends_with(':')
            && ["here", "cleaned", "sure", "output", "corrected"].iter().any(|p| first.starts_with(p))
        {
            lines.remove(0);
        }
    }
    let mut s = lines.join("\n").trim().to_string();
    for (a, b) in [('"', '"'), ('\u{201c}', '\u{201d}')] {
        if s.len() >= 2 && s.starts_with(a) && s.ends_with(b) && (if a == b { s.matches(a).count() } else { s.matches(a).count() + s.matches(b).count() }) <= 2 {
            s = s[a.len_utf8()..s.len() - b.len_utf8()].trim().to_string();
        }
    }
    s
}

/// Remove things Whisper invents on silence or noise.
pub fn strip_hallucinations(text: &str, speech_ms: u64, prompt: &str) -> String {
    let mut s = String::new();
    let mut depth_sq = 0;
    for ch in text.chars() {
        match ch {
            '[' => depth_sq += 1,
            ']' if depth_sq > 0 => depth_sq -= 1,
            _ if depth_sq == 0 => s.push(ch),
            _ => {}
        }
    }
    // (music), (silence), (laughs) ...
    let mut cleaned = String::new();
    let mut rest = s.as_str();
    while let Some(start) = rest.find('(') {
        cleaned.push_str(&rest[..start]);
        match rest[start..].find(')') {
            Some(end) => {
                let inner = rest[start + 1..start + end].to_lowercase();
                let noise = ["music", "silence", "laugh", "applause", "inaudible", "noise", "cough", "sigh", "blank"];
                if !noise.iter().any(|n| inner.contains(n)) {
                    cleaned.push_str(&rest[start..=start + end]);
                }
                rest = &rest[start + end + 1..];
            }
            None => {
                cleaned.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    cleaned.push_str(rest);
    let out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    let norm = words(&out).join(" ");
    let always = [
        "thanks for watching", "thank you for watching", "thank you so much for watching", "please subscribe",
        "subtitles by the amara org community", "like and subscribe",
    ];
    if always.contains(&norm.as_str()) {
        return String::new();
    }
    if speech_ms < 400 && ["you", "thank you", "thanks", "bye", "okay", "so"].contains(&norm.as_str()) {
        return String::new();
    }
    if !prompt.is_empty() && norm == words(prompt).join(" ") {
        return String::new(); // echoed the vocabulary prompt
    }
    out
}

/// Case-insensitive, whole-word snippet expansion.
pub fn expand_snippets(text: &str, snippets: &BTreeMap<String, String>) -> String {
    if snippets.is_empty() {
        return text.to_string();
    }
    // whole utterance is a snippet: "my email." -> the expansion, without the period
    let whole = words(text).join(" ");
    for (k, v) in snippets {
        if words(k).join(" ") == whole {
            return v.clone();
        }
    }
    let mut s = text.to_string();
    for (k, v) in snippets {
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        let lower = s.to_ascii_lowercase();
        let kl = k.to_ascii_lowercase();
        let mut result = String::new();
        let mut last = 0;
        let mut i = 0;
        while let Some(pos) = lower[i..].find(&kl) {
            let start = i + pos;
            let end = start + kl.len();
            let before_ok = start == 0 || !lower[..start].chars().last().unwrap().is_alphanumeric();
            let after_ok = end == lower.len() || !lower[end..].chars().next().unwrap().is_alphanumeric();
            if before_ok && after_ok {
                result.push_str(&s[last..start]);
                result.push_str(v);
                last = end;
            }
            i = end;
        }
        result.push_str(&s[last..]);
        s = result;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_remove_fillers_and_stutters() {
        assert_eq!(rules("um so I I think uh we should go", Style::Default), "So I think we should go.");
        assert_eq!(rules("yes", Style::Default), "Yes");
        assert_eq!(rules("i'm here, um.", Style::Default), "I'm here.");
        assert_eq!(rules("git status", Style::Code), "git status");
        assert_eq!(rules("uh", Style::Default), "");
    }

    #[test]
    fn needs_ai_skips_trivial() {
        assert!(!needs_ai("yes"));
        assert!(!needs_ai("sounds good"));
        assert!(needs_ai("um yes"));
        assert!(needs_ai("let's meet on tuesday at three"));
    }

    #[test]
    fn guard_accepts_real_cleanups() {
        let raw = "um so I think we should uh meet on tuesday no wait wednesday at three pm";
        assert!(accept(raw, "So I think we should meet on Wednesday at 3 PM.", &[]));
        assert!(accept("things to buy are milk eggs and bread", "Things to buy:\n- Milk\n- Eggs\n- Bread", &[]));
        assert!(accept("i dont know if its gonna work", "I don't know if it's gonna work.", &[]));
    }

    #[test]
    fn guard_rejects_answers_and_additions() {
        let raw = "what is the capital of france";
        assert!(!accept(raw, "The capital of France is Paris, a city known for the Eiffel Tower.", &[]));
        assert!(!accept("write me a poem about cats", "Soft paws upon the windowsill, a quiet purr, the world is still.", &[]));
        assert!(!accept("send the report to john by friday please", "", &[]));
        assert!(!accept("send the report to john by friday please ok", "Done.", &[]));
    }

    #[test]
    fn guard_allows_transliteration() {
        assert!(accept("मैं कल आऊंगा", "Main kal aaunga.", &[]));
    }

    #[test]
    fn guard_uses_dictionary() {
        assert!(accept("deploy it to cooper netties", "Deploy it to Kubernetes.", &["Kubernetes".into()]));
    }

    #[test]
    fn sanitize_strips_wrappers() {
        assert_eq!(sanitize_llm("Here is the cleaned text:\nHello there."), "Hello there.");
        assert_eq!(sanitize_llm("\"Hello there.\""), "Hello there.");
        assert_eq!(sanitize_llm("<transcript>Hi</transcript>"), "Hi");
    }

    #[test]
    fn hallucinations_removed() {
        assert_eq!(strip_hallucinations(" Thanks for watching!", 2000, ""), "");
        assert_eq!(strip_hallucinations("[BLANK_AUDIO]", 100, ""), "");
        assert_eq!(strip_hallucinations("Hello (music) world", 2000, ""), "Hello world");
        assert_eq!(strip_hallucinations("Thank you.", 1200, ""), "Thank you.");
        assert_eq!(strip_hallucinations("Thank you.", 200, ""), "");
    }

    #[test]
    fn snippets_expand() {
        let mut m = BTreeMap::new();
        m.insert("my email".to_string(), "me@x.com".to_string());
        assert_eq!(expand_snippets("My email.", &m), "me@x.com");
        assert_eq!(expand_snippets("Send it to my email please.", &m), "Send it to me@x.com please.");
        assert_eq!(expand_snippets("myemailer", &m), "myemailer");
    }
}
