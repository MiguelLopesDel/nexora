//! User-defined transcript corrections ([meeting.corrections]).

use std::collections::BTreeMap;

/// Apply the user's transcript corrections: each key is matched as a whole
/// word or word sequence, ignoring case, so a short key like "vc" never
/// rewrites the middle of another word. A capitalized match keeps its
/// capitalization ("Clod disse" → "Claude disse" with clod=claude).
pub fn apply_corrections(text: &str, corrections: &BTreeMap<String, String>) -> String {
    let mut result = text.to_string();
    for (wrong, right) in corrections {
        result = replace_word_sequence(&result, wrong, right);
    }
    result
}

/// Words are runs of alphanumeric characters plus `'` and `-`, so "gpt-4" and
/// "d'água" match as single words; punctuation around a match is preserved.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '-'
}

fn replace_word_sequence(text: &str, wrong: &str, right: &str) -> String {
    let key: Vec<String> = wrong
        .split(|c| !is_word_char(c))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    if key.is_empty() {
        return text.to_string();
    }

    // Tokenize into alternating word/separator slices.
    let mut tokens: Vec<(&str, bool)> = Vec::new();
    let mut start = 0;
    let mut in_word = None::<bool>;
    for (index, c) in text.char_indices() {
        let word = is_word_char(c);
        if in_word != Some(word) {
            if index > start {
                tokens.push((&text[start..index], in_word == Some(true)));
            }
            start = index;
            in_word = Some(word);
        }
    }
    if text.len() > start {
        tokens.push((&text[start..], in_word == Some(true)));
    }
    let words: Vec<usize> = (0..tokens.len()).filter(|&i| tokens[i].1).collect();

    // Collect non-overlapping matches as token ranges, left to right.
    let mut matches: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i + key.len() <= words.len() {
        let hit = key
            .iter()
            .enumerate()
            .all(|(j, part)| tokens[words[i + j]].0.to_lowercase() == *part);
        if hit {
            matches.push((words[i], words[i + key.len() - 1]));
            i += key.len();
        } else {
            i += 1;
        }
    }
    if matches.is_empty() {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut skip_until = None::<usize>;
    for (index, (slice, _)) in tokens.iter().enumerate() {
        if let Some(end) = skip_until {
            if index <= end {
                continue;
            }
            skip_until = None;
        }
        if let Some((_, end)) = matches.iter().find(|(start, _)| *start == index) {
            let capitalized = slice.chars().next().is_some_and(char::is_uppercase);
            if capitalized && right.chars().next().is_some_and(char::is_lowercase) {
                let mut chars = right.chars();
                if let Some(first) = chars.next() {
                    result.extend(first.to_uppercase());
                    result.push_str(chars.as_str());
                }
            } else {
                result.push_str(right);
            }
            skip_until = Some(*end);
        } else {
            result.push_str(slice);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corrections(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(wrong, right)| (wrong.to_string(), right.to_string()))
            .collect()
    }

    #[test]
    fn corrections_match_whole_words_ignoring_case() {
        let map = corrections(&[("vc", "você")]);
        assert_eq!(
            apply_corrections("Vc viu? vc chegou, VC!", &map),
            "Você viu? você chegou, Você!"
        );
    }

    #[test]
    fn corrections_never_rewrite_inside_words() {
        let map = corrections(&[("ia", "IA")]);
        assert_eq!(
            apply_corrections("a meia dizia que ia", &map),
            "a meia dizia que IA"
        );
    }

    #[test]
    fn corrections_replace_multi_word_sequences() {
        let map = corrections(&[("open a i", "OpenAI")]);
        assert_eq!(
            apply_corrections("falaram da open a i ontem", &map),
            "falaram da OpenAI ontem"
        );
    }

    #[test]
    fn corrections_keep_capitalization_and_punctuation() {
        let map = corrections(&[("clod", "claude")]);
        assert_eq!(
            apply_corrections("Clod, o clod.", &map),
            "Claude, o claude."
        );
    }

    #[test]
    fn corrections_treat_hyphenated_terms_as_one_word() {
        let map = corrections(&[("gpt-4", "GPT-4")]);
        assert_eq!(
            apply_corrections("usei o gpt-4 hoje", &map),
            "usei o GPT-4 hoje"
        );
    }

    #[test]
    fn empty_corrections_leave_text_untouched() {
        assert_eq!(
            apply_corrections("nada muda", &BTreeMap::new()),
            "nada muda"
        );
    }
}
