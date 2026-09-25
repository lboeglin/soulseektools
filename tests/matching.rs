//! Romaji-aware fuzzy matching.

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
// An empty name short-circuits to exactly zero.
#[allow(clippy::float_cmp)]
fn empty_names_are_falsy_and_never_match() {
    assert!(!forms("").is_present());
    assert_eq!(similarity(&forms(""), &forms("anything")), 0.0);
}

#[test]
fn kanji_matches_its_romanised_filename() {
    assert!(
        similarity(&forms("\u{5343}\u{672c}\u{685c}"), &forms("Senbonzakura"))
            >= thresholds().title_strong
    );
}

#[test]
fn two_kanji_spellings_sharing_a_reading_match_through_romaji() {
    // Only matches because romaji is compared against romaji too.
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
fn a_name_and_its_romanisation_share_a_blocking_key() {
    let kanji = forms("\u{5343}\u{672c}\u{685c}");
    let romanised = forms("Senbonzakura");
    assert!(keys(&[&kanji])
        .intersection(&keys(&[&romanised]))
        .next()
        .is_some());
}

#[test]
fn candidate_pairs_skips_almost_every_comparison_in_a_distinct_library() {
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
