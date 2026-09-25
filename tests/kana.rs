//! Hepburn romanisation of kana.

use stk::text::kana::kana_to_romaji;

fn assert_romanises(cases: &[(&str, &str)]) {
    for (kana, romaji) in cases {
        assert_eq!(kana_to_romaji(kana), *romaji, "kana_to_romaji({kana:?})");
    }
}

#[test]
fn long_vowels_are_written_out() {
    assert_romanises(&[
        ("ロウワー", "rouwaa"),
        ("きゃー", "kyaa"),
        ("おう", "ou"),
        ("んー", "nn"),
        ("ー", ""),
    ]);
}

#[test]
fn a_small_tsu_doubles_the_consonant_after_it() {
    assert_romanises(&[
        ("ラビット", "rabitto"),
        ("まっちゃ", "matcha"),
        ("っしゃ", "ssha"),
        ("わっれっちゃう", "warretchau"),
    ]);
}

#[test]
fn only_the_named_pairs_read_as_one_syllable() {
    assert_romanises(&[
        ("ファントム", "fantomu"),
        ("ヴィータ", "viita"),
        ("フェ", "fe"),
        ("チェ", "che"),
        ("ディ", "di"),
        ("ジャ", "ja"),
        ("ティ", "tei"),
        ("ジェ", "jie"),
        ("ウィ", "ui"),
    ]);
}
