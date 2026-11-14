//! The installed tab's search, as the original's: words in the title, a typo
//! or two forgiven, and a few words that are tags rather than words.

/// The words of a search, as the original reads them: lower case, without
/// "?", "." and "!", with "hdr", "3d" and "puppet" (warp) taken as tags.
pub(super) fn search_words(text: &str) -> (String, Vec<&'static str>) {
    let mut text: String = text.to_lowercase().chars().filter(|c| !matches!(c, '?' | '.' | '!')).collect();
    let mut tags = Vec::new();
    for (words, tag) in [
        (&["hdr"][..], "HDR"),
        (&["3d"][..], "3D"),
        (&["puppet warp", "puppetwarp", "puppet"][..], "Puppet Warp"),
    ] {
        for word in words {
            let found = text.split_whitespace().collect::<Vec<_>>().join(" ");
            let padded = format!(" {found} ");
            if padded.contains(&format!(" {word} ")) {
                text = padded.replace(&format!(" {word} "), " ").trim().to_owned();
                if !tags.contains(&tag) {
                    tags.push(tag);
                }
            }
        }
    }
    (text.trim().to_owned(), tags)
}

/// Whether a title answers a search: it holds the words, or is within two
/// edits of them (a typo, two letters swapped), as in the original.
pub(super) fn title_matches(title: &str, words: &str) -> bool {
    if words.is_empty() {
        return true;
    }
    let title: String = title.to_lowercase().chars().filter(|c| !matches!(c, '?' | '.' | '!')).collect();
    title.contains(words) || edits(&title, words) < 3
}

/// The Damerau-Levenshtein distance (optimal string alignment) of two texts.
fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_takes_tags_and_typos() {
        assert_eq!(search_words("Rain HDR!"), ("rain".to_owned(), vec!["HDR"]));
        assert_eq!(search_words("puppet warp girl"), ("girl".to_owned(), vec!["Puppet Warp"]));
        assert!(title_matches("Purple Space", "space"));
        assert!(title_matches("Rain", "rian"));
        assert!(!title_matches("Purple Space", "forest"));
        assert!(title_matches("anything", ""));
        assert_eq!(edits("abc", "acb"), 1);
        assert_eq!(edits("", "abc"), 3);
    }

    #[test]
    fn a_search_of_tags_alone_has_no_words() {
        assert_eq!(search_words("3D hdr"), (String::new(), vec!["HDR", "3D"]));
        assert_eq!(search_words("  Puppet   "), (String::new(), vec!["Puppet Warp"]));
        // A tag word inside another word is a word.
        assert_eq!(search_words("hdrish"), ("hdrish".to_owned(), vec![]));
    }

    #[test]
    fn titles_are_read_without_their_punctuation() {
        assert!(title_matches("What?! A Cat.", "what a cat"));
        assert!(title_matches("Ocean", "ocaen"));
        assert!(!title_matches("Ocean", "forest"));
    }
}
