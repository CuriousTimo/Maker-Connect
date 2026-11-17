//! Port of the parts of OctoPrint 1.11.8 `src/octoprint/util/text.py` (and what it calls,
//! `vendor/awesome_slugify/main.py`, and the third-party `emoji` and `unidecode` packages) that
//! `util/files.py:get_dos_filename` needs.
//!
//! Original: Copyright (C) 2020 The OctoPrint Project, AGPL-3.0.
//!
//! NOT PORTED (third-party data we do not have in `reference/`): `emoji.demojize` and
//! `unidecode.unidecode` for text that contains a non-ASCII character. They panic with
//! `unimplemented!`. For ASCII-only text both are the identity (`unidecode` returns an ASCII
//! string as it is; no key of `emoji.EMOJI_DATA` is pure ASCII, checked on the `emoji` package
//! OctoPrint 1.11.8 installs), and the rest of `Slugify.__call__` is ported for that case.

use super::comm::py_strip;

/// `util/text.py:13`
const _UNICODE_VARIATIONS_START: char = '\u{fe00}';
const _UNICODE_VARIATIONS_END: char = '\u{fe0f}';

/// `util/text.py:36 remove_unicode_variations`: `re.compile("[︀-️]").sub("", text)`
pub fn remove_unicode_variations(text: &str) -> String {
    text.chars()
        .filter(|c| !(_UNICODE_VARIATIONS_START..=_UNICODE_VARIATIONS_END).contains(c))
        .collect()
}

/// `util/text.py:16 demojify`: `to_unicode`, remove variations, `emoji.demojize(text,
/// delimiters=("", ""))` (identity for ASCII text, see the module doc).
pub fn demojify(text: &str) -> String {
    let text = remove_unicode_variations(text);
    if !text.is_ascii() {
        unimplemented!(
            "emoji.demojize (third-party `emoji` package), called at util/text.py:20, for non-ASCII text"
        );
    }
    text
}

/// `vendor/awesome_slugify/main.py:56 Slugify()` with `safe_chars` set and every other argument at
/// its default (`separator="-"`, `to_lower=False`, `max_length=None`, no stop words), called on
/// `text` (`__call__`, main.py:144). ASCII text only (see the module doc).
fn slugify(text: &str, safe_chars: &str) -> String {
    if !text.is_ascii() {
        unimplemented!(
            "unidecode.unidecode (third-party `unidecode` package), called at vendor/awesome_slugify/main.py:171, for non-ASCII text"
        );
    }

    // text_parts = self.upper_to_upper_letters_re.split(text); every part goes through
    // pretranslate (None: identity), translate (unidecode: identity for ASCII) and, for the
    // matched parts (upper case letters only), `.upper()` (identity): `u''.join(text_parts)` is
    // `text`.

    // main.py:116 sanitize: `apostrophe_is_not_safe = "'" not in safe_chars`
    let mut text = text.to_string();
    if !safe_chars.contains('\'') {
        text = py_strip(&text.replace('\'', "")).to_string();
    }

    // `unwanted_chars_re = [^\p{AlNum}<safe_chars>]+`; `words = filter(None, re.split(text))`
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || safe_chars.contains(c)))
        .filter(|w| !w.is_empty())
        .collect();

    // join_words(words, separator, max_length=None): `separator.join(words)`
    words.join("-")
}

/// `util/text.py:21 sanitize(text, safe_chars="-_.", demoji=True)`
///
/// Sanitizes text by running it through slugify and optionally emoji translating.
pub fn sanitize(text: &str, safe_chars: &str, demoji: bool) -> String {
    let text = if demoji {
        demojify(text)
    } else {
        text.to_string()
    };
    slugify(&text, safe_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docstring examples of `util/text.py:sanitize` (the ASCII ones; the `❤` ones need
    /// `emoji`, not ported)
    #[test]
    fn test_sanitize_docstring_ascii() {
        assert_eq!("Hello-World", sanitize("Hello World!", "-_.", true));
        assert_eq!("Hello World", sanitize("Hello World!", "-_. ", true));
    }

    /// ours: `remove_unicode_variations`, `'` handling, empty result
    #[test]
    fn ours_variations_and_apostrophe() {
        assert_eq!("ab", remove_unicode_variations("a\u{fe0f}b\u{fe00}"));
        assert_eq!("Its-fine", sanitize("It's fine", "-_.", true));
        assert_eq!("It's-fine", sanitize("It's fine", "-_.'", true));
        assert_eq!("", sanitize("!!!", "-_.", true));
    }
}
