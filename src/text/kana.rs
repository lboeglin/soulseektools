//! Hepburn romanisation of kana.
//!
//! A long vowel is written out: `おう` is `ou` and `ー` repeats the letter before
//! it. `っ` doubles the next consonant (not `n`, `m` or `w`), else is `tsu`.
//! `ん` is `n` with no apostrophe, which `normalize` would split on. Only the
//! pairs [`digraph`] names are one syllable, so `ティ` is `tei`.

/// Romanise every kana in `text`, passing anything else through unchanged.
#[must_use]
pub fn kana_to_romaji(text: &str) -> String {
    let kana: Vec<char> = text.chars().map(to_hiragana).collect();
    let mut romaji = String::with_capacity(text.len() * 2);
    let mut rest = kana.as_slice();

    while let Some(&first) = rest.first() {
        if first == 'っ' {
            match syllable_at(&rest[1..]) {
                Some((next, _)) if doubles(rest[1], &next) => {
                    romaji.push_str(if next.starts_with("ch") {
                        "t"
                    } else {
                        &next[..1]
                    });
                }
                _ => romaji.push_str("tsu"),
            }
            rest = &rest[1..];
        } else if first == 'ー' {
            if let Some(last) = romaji.chars().last().filter(|&c| is_vowel(c) || c == 'n') {
                romaji.push(last);
            }
            rest = &rest[1..];
        } else if let Some((syllable, width)) = syllable_at(rest) {
            romaji.push_str(&syllable);
            rest = &rest[width..];
        } else {
            romaji.push(first);
            rest = &rest[1..];
        }
    }
    romaji
}

fn to_hiragana(c: char) -> char {
    match c {
        'ァ'..='ヶ' => char::from_u32(u32::from(c) - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'i' | 'u' | 'e' | 'o')
}

fn is_small(kana: char) -> bool {
    matches!(
        kana,
        'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ'
    )
}

/// Whether a `っ` before `kana`, read as `syllable`, doubles its consonant.
fn doubles(kana: char, syllable: &str) -> bool {
    !is_small(kana) && syllable.starts_with(|c: char| !is_vowel(c) && !matches!(c, 'n' | 'm' | 'w'))
}

/// The syllable `kana` starts with, and how many kana it spans.
fn syllable_at(kana: &[char]) -> Option<(String, usize)> {
    let first = *kana.first()?;
    if let Some(pair) = kana.get(1).and_then(|&small| digraph(first, small)) {
        return Some((pair, 2));
    }
    single(first).map(|syllable| (syllable.to_owned(), 1))
}

/// The pairs read as one syllable: a small `ゃゅょ` after an `i`-row kana, a
/// small vowel after `ふ` or `ゔ`, and `ちぇ` and `でぃ`.
fn digraph(base: char, small: char) -> Option<String> {
    let vowel = match small {
        'ゃ' | 'ぁ' => 'a',
        'ぃ' => 'i',
        'ゅ' => 'u',
        'ぇ' => 'e',
        'ょ' | 'ぉ' => 'o',
        _ => return None,
    };
    let base_romaji = single(base)?;
    let stem = &base_romaji[..base_romaji.len() - 1];

    let glide = match (base, small) {
        ('き' | 'ぎ' | 'に' | 'ひ' | 'び' | 'ぴ' | 'み' | 'り', 'ゃ' | 'ゅ' | 'ょ') => {
            "y"
        }
        ('し' | 'じ' | 'ち' | 'ぢ', 'ゃ' | 'ゅ' | 'ょ')
        | ('ふ' | 'ゔ', 'ぁ' | 'ぃ' | 'ぇ' | 'ぉ')
        | ('ち', 'ぇ')
        | ('で', 'ぃ') => "",
        _ => return None,
    };
    Some(format!("{stem}{glide}{vowel}"))
}

fn single(kana: char) -> Option<&'static str> {
    Some(match kana {
        'あ' | 'ぁ' => "a",
        'い' | 'ぃ' | 'ゐ' => "i",
        'う' | 'ぅ' => "u",
        'え' | 'ぇ' | 'ゑ' => "e",
        'お' | 'ぉ' => "o",
        'か' | 'ゕ' => "ka",
        'き' => "ki",
        'く' => "ku",
        'け' | 'ゖ' => "ke",
        'こ' => "ko",
        'が' => "ga",
        'ぎ' => "gi",
        'ぐ' => "gu",
        'げ' => "ge",
        'ご' => "go",
        'さ' => "sa",
        'し' => "shi",
        'す' => "su",
        'せ' => "se",
        'そ' => "so",
        'ざ' => "za",
        'じ' | 'ぢ' => "ji",
        'ず' | 'づ' => "zu",
        'ぜ' => "ze",
        'ぞ' => "zo",
        'た' => "ta",
        'ち' => "chi",
        'つ' => "tsu",
        'て' => "te",
        'と' => "to",
        'だ' => "da",
        'で' => "de",
        'ど' => "do",
        'な' => "na",
        'に' => "ni",
        'ぬ' => "nu",
        'ね' => "ne",
        'の' => "no",
        'は' => "ha",
        'ひ' => "hi",
        'ふ' => "fu",
        'へ' => "he",
        'ほ' => "ho",
        'ば' => "ba",
        'び' => "bi",
        'ぶ' => "bu",
        'べ' => "be",
        'ぼ' => "bo",
        'ぱ' => "pa",
        'ぴ' => "pi",
        'ぷ' => "pu",
        'ぺ' => "pe",
        'ぽ' => "po",
        'ま' => "ma",
        'み' => "mi",
        'む' => "mu",
        'め' => "me",
        'も' => "mo",
        'や' | 'ゃ' => "ya",
        'ゆ' | 'ゅ' => "yu",
        'よ' | 'ょ' => "yo",
        'ら' => "ra",
        'り' => "ri",
        'る' => "ru",
        'れ' => "re",
        'ろ' => "ro",
        'わ' | 'ゎ' => "wa",
        'を' => "wo",
        'ん' => "n",
        'ゔ' => "vu",
        'ヷ' => "va",
        'ヸ' => "vi",
        'ヹ' => "ve",
        'ヺ' => "vo",
        'ゟ' => "yori",
        'ヿ' => "koto",
        _ => return None,
    })
}
