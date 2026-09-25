//! Hepburn romanisation of kana.

use stk::text::kana::kana_to_romaji;

fn assert_romanises(cases: &[(&str, &str)]) {
    for (kana, romaji) in cases {
        assert_eq!(kana_to_romaji(kana), *romaji, "kana_to_romaji({kana:?})");
    }
}

#[test]
fn hiragana_and_katakana_spell_the_same() {
    assert_romanises(&[
        ("さくら", "sakura"),
        ("サクラ", "sakura"),
        ("しちつふ", "shichitsufu"),
    ]);
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
fn a_small_tsu_with_nothing_to_double_is_tsu() {
    assert_romanises(&[
        ("めっっっ", "metsutsutsu"),
        ("んっあっ", "ntsuatsu"),
        ("っな", "tsuna"),
        ("っー", "tsuu"),
    ]);
}

#[test]
fn n_takes_no_apostrophe() {
    assert_romanises(&[("ロシンユウカイ", "roshinyuukai"), ("きんえん", "kinen")]);
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

#[test]
fn text_that_is_not_kana_passes_through() {
    assert_romanises(&[("25じ", "25ji"), ("abc", "abc"), ("ゝ", "ゝ")]);
}
