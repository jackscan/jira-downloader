use globset::{GlobBuilder, GlobMatcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Glob,
    Substring,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    Applied,
    NoEligibleMatch,
    EmptyInput,
    InvalidGlob,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterPattern {
    pub raw: String,
    pub kind: PatternKind,
}

impl FilterPattern {
    pub fn from_input(input: &str) -> Self {
        let trimmed = input.trim();
        let kind = if trimmed.is_empty() {
            PatternKind::Empty
        } else if trimmed.contains('*') || trimmed.contains('?') {
            PatternKind::Glob
        } else {
            PatternKind::Substring
        };

        Self {
            raw: trimmed.to_string(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentQueueState {
    NotDownloaded,
    Failed,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterApplyResult {
    pub matched_total: usize,
    pub eligible_total: usize,
    pub queued_total: usize,
    pub skipped_total: usize,
    pub outcome: FilterOutcome,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterInputState {
    pub is_active: bool,
    pub draft: String,
    pub preview_matches: Vec<usize>,
    pub compile_error: Option<String>,
    pub cursor_index: usize,
}

impl FilterInputState {
    pub fn activate(&mut self) {
        self.is_active = true;
        self.preview_matches.clear();
        self.compile_error = None;
        self.cursor_index = self.draft.len();
    }

    pub fn cancel(&mut self) {
        self.is_active = false;
        self.draft.clear();
        self.preview_matches.clear();
        self.compile_error = None;
        self.cursor_index = 0;
    }
}

pub fn is_queue_eligible(state: AttachmentQueueState) -> bool {
    matches!(
        state,
        AttachmentQueueState::NotDownloaded | AttachmentQueueState::Failed
    )
}

pub fn no_eligible_match_message(pattern: &str) -> String {
    format!("No eligible attachments matched '{}'", pattern)
}

pub fn build_apply_result(
    pattern: &str,
    matched_total: usize,
    eligible_total: usize,
    queued_total: usize,
    skipped_total: usize,
) -> FilterApplyResult {
    if matched_total == 0 || eligible_total == 0 {
        return FilterApplyResult {
            matched_total,
            eligible_total,
            queued_total,
            skipped_total,
            outcome: FilterOutcome::NoEligibleMatch,
            message: no_eligible_match_message(pattern),
        };
    }

    let message = if skipped_total == 0 {
        format!("Queued {} attachment(s) for '{}'", queued_total, pattern)
    } else {
        format!(
            "Queued {} attachment(s) for '{}' ({} skipped)",
            queued_total, pattern, skipped_total
        )
    };

    FilterApplyResult {
        matched_total,
        eligible_total,
        queued_total,
        skipped_total,
        outcome: FilterOutcome::Applied,
        message,
    }
}

pub fn empty_input_result() -> FilterApplyResult {
    FilterApplyResult {
        matched_total: 0,
        eligible_total: 0,
        queued_total: 0,
        skipped_total: 0,
        outcome: FilterOutcome::EmptyInput,
        message: "Filter pattern is empty. No changes made.".to_string(),
    }
}

pub fn invalid_glob_result(pattern: &str, errmsg: &str) -> FilterApplyResult {
    FilterApplyResult {
        matched_total: 0,
        eligible_total: 0,
        queued_total: 0,
        skipped_total: 0,
        outcome: FilterOutcome::InvalidGlob,
        message: format!("Invalid glob pattern '{}': {}", pattern, errmsg),
    }
}

pub fn matching_indices<'a, I>(pattern: &FilterPattern, filenames: I) -> Result<Vec<usize>, String>
where
    I: IntoIterator<Item = &'a str>,
{
    match pattern.kind {
        PatternKind::Empty => Ok(Vec::new()),
        PatternKind::Substring => {
            let needle = pattern.raw.to_lowercase();
            Ok(filenames
                .into_iter()
                .enumerate()
                .filter_map(|(index, filename)| {
                    filename.to_lowercase().contains(&needle).then_some(index)
                })
                .collect())
        }
        PatternKind::Glob => {
            let matcher = compile_glob_matcher(&pattern.raw)?;
            Ok(filenames
                .into_iter()
                .enumerate()
                .filter_map(|(index, filename)| matcher.is_match(filename).then_some(index))
                .collect())
        }
    }
}

fn compile_glob_matcher(pattern: &str) -> Result<GlobMatcher, String> {
    validate_glob_pattern(pattern)?;

    let glob = GlobBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|err| err.to_string())?;
    Ok(glob.compile_matcher())
}

fn validate_glob_pattern(pattern: &str) -> Result<(), String> {
    let mut class_depth = 0usize;
    let mut escaped = false;

    for ch in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '[' => class_depth += 1,
            ']' => {
                if class_depth == 0 {
                    return Err("unmatched closing ']'".to_string());
                }
                class_depth -= 1;
            }
            _ => {}
        }
    }

    if class_depth > 0 {
        return Err("unclosed character class".to_string());
    }

    Ok(())
}
