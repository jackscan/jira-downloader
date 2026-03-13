#[path = "../src/filter.rs"]
mod filter;

use filter::{AttachmentQueueState, FilterInputState, FilterOutcome, FilterPattern, PatternKind};
use std::time::{Duration, Instant};

#[test]
fn parses_pattern_kind_from_input() {
    assert_eq!(FilterPattern::from_input("   ").kind, PatternKind::Empty);
    assert_eq!(FilterPattern::from_input("*.log").kind, PatternKind::Glob);
    assert_eq!(
        FilterPattern::from_input("report").kind,
        PatternKind::Substring
    );
}

#[test]
fn matches_glob_case_insensitive() {
    let pattern = FilterPattern::from_input("*.LOG");
    let matches = filter::matching_indices(&pattern, ["report.log", "notes.txt", "RUN.LOG"])
        .expect("glob must compile");
    assert_eq!(matches, vec![0, 2]);
}

#[test]
fn matches_substring_case_insensitive() {
    let pattern = FilterPattern::from_input("report");
    let matches =
        filter::matching_indices(&pattern, ["Report-2025.pdf", "summary.txt", "xREPORTx.bin"])
            .expect("substring matching must succeed");
    assert_eq!(matches, vec![0, 2]);
}

#[test]
fn invalid_glob_returns_error() {
    let pattern = FilterPattern::from_input("*[abc");
    let result = filter::matching_indices(&pattern, ["a.txt", "b.txt"]);
    assert!(result.is_err());
}

#[test]
fn empty_pattern_returns_no_matches() {
    let pattern = FilterPattern::from_input("  ");
    let matches = filter::matching_indices(&pattern, ["a.txt", "b.txt"])
        .expect("empty pattern should not fail");
    assert!(matches.is_empty());
}

#[test]
fn applies_no_eligible_match_message() {
    let msg = filter::no_eligible_match_message("*.zip");
    assert_eq!(msg, "No eligible attachments matched '*.zip'");
}

#[test]
fn apply_result_marks_no_eligible_match() {
    let result = filter::build_apply_result("*.zip", 3, 0, 0, 3);
    assert_eq!(result.outcome, FilterOutcome::NoEligibleMatch);
    assert_eq!(result.message, "No eligible attachments matched '*.zip'");
}

#[test]
fn empty_and_invalid_outcomes_are_constructed() {
    let empty = filter::empty_input_result();
    assert_eq!(empty.outcome, FilterOutcome::EmptyInput);

    let invalid = filter::invalid_glob_result("*[abc", "unclosed character class");
    assert_eq!(invalid.outcome, FilterOutcome::InvalidGlob);
}

#[test]
fn queue_eligibility_helper_handles_states() {
    assert!(filter::is_queue_eligible(
        AttachmentQueueState::NotDownloaded
    ));
    assert!(filter::is_queue_eligible(AttachmentQueueState::Failed));
    assert!(!filter::is_queue_eligible(AttachmentQueueState::Other));
}

#[test]
fn cancel_resets_filter_draft_and_keeps_external_state() {
    let mut input = FilterInputState::default();
    input.activate();
    input.draft = "report".to_string();
    input.preview_matches = vec![1, 4];
    input.compile_error = Some("invalid glob".to_string());
    input.cursor_index = input.draft.len();

    let states = vec![
        AttachmentQueueState::NotDownloaded,
        AttachmentQueueState::Other,
        AttachmentQueueState::Failed,
    ];
    let states_before = states.clone();

    input.cancel();

    assert!(!input.is_active);
    assert!(input.draft.is_empty());
    assert!(input.preview_matches.is_empty());
    assert!(input.compile_error.is_none());
    assert_eq!(input.cursor_index, 0);
    assert_eq!(states, states_before);
}

#[test]
fn performance_stays_under_100ms_for_200_entries() {
    let mut filenames = Vec::with_capacity(200);
    for i in 0..200 {
        if i % 5 == 0 {
            filenames.push(format!("report-{:03}.log", i));
        } else {
            filenames.push(format!("attachment-{:03}.txt", i));
        }
    }

    let pattern = FilterPattern::from_input("report");

    let start = Instant::now();
    let matches = filter::matching_indices(&pattern, filenames.iter().map(|s| s.as_str()))
        .expect("substring matching should succeed");
    let elapsed = start.elapsed();

    assert!(!matches.is_empty());
    assert!(
        elapsed < Duration::from_millis(100),
        "matching took {:?}, expected < 100ms",
        elapsed
    );
}
