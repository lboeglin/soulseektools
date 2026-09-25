//! Romaji-aware fuzzy matching of artist and title names.

use std::collections::HashSet;

use stk::matching::{
    blocking_keys, candidate_pairs, score_track, similarity, MatchThresholds, NameForms,
};

fn forms(text: &str) -> NameForms {
    NameForms::of(text)
}

fn thresholds() -> MatchThresholds {
    MatchThresholds::default()
}

fn keys(forms: &[&NameForms]) -> HashSet<String> {
    blocking_keys(forms)
}

#[test]
fn name_forms_carry_the_raw_normalised_and_romanised_spellings() {
    let name = forms("\u{5343}\u{672c}\u{685c}");
    assert_eq!(name.raw, "\u{5343}\u{672c}\u{685c}");
    assert_eq!(name.normalized, "\u{5343}\u{672c}\u{685c}");
    assert_eq!(name.romaji, "senbonzakura");
}

#[test]
// An empty name short-circuits to a literal zero rather than a computed score,
// so exact equality is the assertion.
#[allow(clippy::float_cmp)]
fn empty_names_are_falsy_and_never_match() {
    assert!(!forms("").is_present());
    assert_eq!(similarity(&forms(""), &forms("anything")), 0.0);
}

#[test]
fn kanji_matches_its_romanised_filename() {
    // The case the whole romaji layer exists for: the export says 千本桜, the
    // file on disk is Senbonzakura.flac.
    assert!(
        similarity(&forms("\u{5343}\u{672c}\u{685c}"), &forms("Senbonzakura"))
            >= thresholds().title_strong
    );
}

#[test]
fn two_kanji_spellings_sharing_a_reading_match_through_romaji() {
    // Neither side is romanised, so this only works because romaji is compared
    // against romaji as well as against the normalised form.
    assert!(
        similarity(
            &forms("\u{521d}\u{97f3}\u{30df}\u{30af}"),
            &forms("\u{306f}\u{3064}\u{306d}\u{307f}\u{304f}")
        ) >= thresholds().title_strong
    );
}

#[test]
fn containment_rescues_a_truncated_title() {
    assert!(similarity(&forms("Senbonzakura"), &forms("Senbonzakura -Long Ver-")) >= 0.85);
}

#[test]
fn reordered_words_still_match() {
    assert!(
        similarity(
            &forms("Hatsune Miku Senbonzakura"),
            &forms("Senbonzakura Hatsune Miku")
        ) >= 0.85
    );
}

#[test]
fn unrelated_titles_score_low() {
    assert!(similarity(&forms("Night of Knights"), &forms("Heian Alien")) < thresholds().title);
}

#[test]
fn matching_artist_and_title_are_accepted() {
    let score = score_track(
        &forms("DECO*27"),
        &forms("Roki"),
        &forms("DECO_27"),
        &forms("Roki"),
        &thresholds(),
    )
    .expect("a match");

    assert!((score.artist - 1.0).abs() < 1e-9);
    assert!((score.title - 1.0).abs() < 1e-9);
    assert!((score.combined - 1.0).abs() < 1e-9);
}

#[test]
fn a_strong_title_carries_a_weak_artist() {
    assert!(score_track(
        &forms("Mikito P"),
        &forms("Roki"),
        &forms("mikitoP"),
        &forms("Roki"),
        &thresholds(),
    )
    .is_some());
}

#[test]
fn a_strong_artist_never_carries_a_weak_title() {
    assert!(score_track(
        &forms("DECO*27"),
        &forms("Roki"),
        &forms("DECO*27"),
        &forms("Yowamushi Montblanc"),
        &thresholds(),
    )
    .is_none());
}

#[test]
fn an_uncertain_pair_returns_none_rather_than_a_low_score() {
    assert!(score_track(
        &forms("beatMARIO"),
        &forms("Night of Knights"),
        &forms("ZUN"),
        &forms("Heian Alien"),
        &thresholds(),
    )
    .is_none());
}

#[test]
fn the_title_is_weighted_above_the_artist() {
    let thresholds = thresholds();
    assert!(thresholds.title_weight() > thresholds.artist_weight);
    assert!((thresholds.title_weight() + thresholds.artist_weight - 1.0).abs() < 1e-9);
}

/// The artist 須田景凪 romanises to "sudakeinagi", while the producer romanises
/// the same name as "Suda Keina". Transliteration cannot know that, which is
/// why the artist half of a match is only ever required to clear `artist_weak`.
#[test]
fn artist_readings_can_diverge_but_a_strong_title_still_matches() {
    let kanji = forms("\u{9808}\u{7530}\u{666f}\u{51ea}");
    let romaji_alias = forms("Suda Keina");
    let thresholds = thresholds();

    assert!(similarity(&kanji, &romaji_alias) < 1.0);
    assert!(similarity(&kanji, &romaji_alias) >= thresholds.artist_weak);

    let title = forms("\u{30b7}\u{30e3}\u{30eb}\u{30eb}");
    let matched = score_track(
        &kanji,
        &title,
        &romaji_alias,
        &forms("Sharuru"),
        &thresholds,
    )
    .expect("a match");
    assert!(matched.title >= thresholds.title_strong);
}

#[test]
fn a_name_and_its_romanisation_share_a_blocking_key() {
    let kanji = forms("\u{5343}\u{672c}\u{685c}");
    let romanised = forms("Senbonzakura");
    assert!(keys(&[&kanji])
        .intersection(&keys(&[&romanised]))
        .next()
        .is_some());
}

#[test]
fn unrelated_names_share_no_blocking_key() {
    let left = forms("Night of Knights");
    let right = forms("Heian Alien");
    assert!(keys(&[&left])
        .intersection(&keys(&[&right]))
        .next()
        .is_none());
}

#[test]
fn keys_combine_every_supplied_name() {
    let artist = forms("DECO*27");
    let title = forms("Roki");
    let combined = keys(&[&artist, &title]);
    assert!(keys(&[&title]).is_subset(&combined));
}

#[test]
fn candidate_pairs_yields_only_names_sharing_a_key() {
    let entries = [
        forms("Senbonzakura"),
        forms("\u{5343}\u{672c}\u{685c}"),
        forms("Heian Alien"),
    ];
    let keyed: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, name)| (index, keys(&[name])))
        .collect();

    assert_eq!(candidate_pairs(&keyed), vec![(0, 1)]);
}

#[test]
fn candidate_pairs_yields_each_pair_once() {
    let entries = [
        forms("Night of Knights"),
        forms("Night of Knights (Extended)"),
    ];
    let keyed: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, name)| (index, keys(&[name])))
        .collect();

    assert_eq!(candidate_pairs(&keyed), vec![(0, 1)]);
}

#[test]
fn candidate_pairs_skips_almost_every_comparison_in_a_distinct_library() {
    // Blocking has to turn the quadratic scan into a near-linear one over names
    // that share nothing.
    let mut state = 20_260_830_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (state >> 33) as usize
    };

    let entries: Vec<NameForms> = (0..200)
        .map(|_| {
            let name: String = (0..14)
                .map(|_| char::from(b'a' + u8::try_from(next() % 26).unwrap()))
                .collect();
            forms(&name)
        })
        .collect();
    let keyed: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(index, name)| (index, keys(&[name])))
        .collect();

    let exhaustive = 200 * 199 / 2;
    assert!(candidate_pairs(&keyed).len() < exhaustive / 100);
}
