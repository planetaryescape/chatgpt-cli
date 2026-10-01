//! `withFilters`: `toFilter` and `applyJevFiltersAndLimit` from the TS CLI's
//! `src/commands/select.ts` @ 1b8c950, with its messages and its order of
//! checks.

use chatgpt_protocol::Filter;
use chatgpt_store::{IndexFilter, IndexedConversation, JudgmentRow};
use fancy_regex::Regex;

use crate::js;
use crate::policy::{Judged, PolicyError, Profile, Verdict};

const BRAINSTORM_KINDS: [&str; 4] = ["writing", "sermon", "product", "other"];
const SUGGESTIONS: [&str; 3] = ["delete", "archive", "keep"];
const DAY_MS: i64 = 86_400_000;

/// A filter the TS CLI rejects, with its message.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct InvalidFilter(pub String);

/// The index part of the filter (`toFilter`), and the title regex.
pub struct Selection {
    pub index: IndexFilter,
    pub title: Option<Regex>,
}

/// `parseAge`: `30d`, `12w`, `6m`, `2y` before `now_ms`.
fn parse_age(value: &str, now_ms: i64) -> Result<i64, InvalidFilter> {
    let invalid = || {
        InvalidFilter(format!(
            "Invalid age \"{value}\". Use a number and unit, e.g. 30d, 12w, 6m, 2y."
        ))
    };
    let (digits, unit) = value.split_at(value.len().saturating_sub(1));
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let unit_days = match unit {
        "d" => 1.0,
        "w" => 7.0,
        "m" => 30.0,
        "y" => 365.0,
        _ => return Err(invalid()),
    };
    // As JS numbers: a huge count makes an invalid date, not an overflow.
    let days = js::number(digits) * unit_days;
    Ok(now_ms - (days * DAY_MS as f64).min(i64::MAX as f64) as i64)
}

fn parse_date(value: &str) -> Result<i64, InvalidFilter> {
    js::parse_date(value)
        .ok_or_else(|| InvalidFilter(format!("Invalid date \"{value}\". Use YYYY-MM-DD.")))
}

fn iso(millis: i64) -> Result<String, InvalidFilter> {
    js::iso_from_millis(millis).ok_or_else(|| InvalidFilter("Invalid time value".into()))
}

/// A set option, as JS truthiness has it: `--title ""` sets nothing.
fn given(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// `toFilter`. When both an age and a date bound are given, the stricter
/// one wins.
pub fn selection(filter: &Filter, now_ms: i64) -> Result<Selection, InvalidFilter> {
    let older = given(&filter.older_than)
        .map(|age| parse_age(age, now_ms))
        .transpose()?;
    let before = given(&filter.before).map(parse_date).transpose()?;
    let newer = given(&filter.newer_than)
        .map(|age| parse_age(age, now_ms))
        .transpose()?;
    let after = given(&filter.after).map(parse_date).transpose()?;
    let earliest = match (older, before) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let latest = match (newer, after) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    let title = given(&filter.title)
        .map(|pattern| {
            js::regex(pattern, true).map_err(|error| {
                InvalidFilter(format!("Invalid regular expression: /{pattern}/i: {error}"))
            })
        })
        .transpose()?;
    Ok(Selection {
        index: IndexFilter {
            archived: (!filter.all).then_some(filter.archived),
            // `list` and `stats` include pinned chats.
            include_pinned: true,
            updated_before: earliest.map(iso).transpose()?,
            updated_after: latest.map(iso).transpose()?,
        },
        title,
    })
}

/// The title regex against the display title, then the original one.
pub fn title_matches(title: Option<&Regex>, chat: &IndexedConversation) -> bool {
    let Some(regex) = title else {
        return true;
    };
    // A regex that runs out of backtracking budget matches nothing.
    regex.is_match(chat.display_title()).unwrap_or(false)
        || regex.is_match(&chat.title).unwrap_or(false)
}

/// `--suggest`, `--topic` and `--brainstorm`, validated, and `--limit`.
pub struct JevFilter<'a> {
    limit: Option<usize>,
    suggest: Option<&'a str>,
    topic: Option<&'a str>,
    /// `Some(None)`: any brainstorm.
    brainstorm: Option<Option<&'a str>>,
}

/// The first half of `applyJevFiltersAndLimit`: validate in the TS CLI's
/// order.
pub fn jev_filter<'a>(
    filter: &'a Filter,
    profile: &Profile,
) -> Result<JevFilter<'a>, InvalidFilter> {
    let limit = match given(&filter.limit) {
        None => None,
        Some(raw) => {
            let limit = js::number(raw);
            if !(limit.is_finite() && limit.fract() == 0.0 && limit >= 1.0) {
                return Err(InvalidFilter(
                    "--limit must be a positive whole number.".into(),
                ));
            }
            Some(limit.min(usize::MAX as f64) as usize)
        }
    };
    let suggest = given(&filter.suggest);
    let topic = given(&filter.topic);
    // Commander gives `true` for a bare --brainstorm; the CLI sends "".
    let brainstorm = filter
        .brainstorm
        .as_deref()
        .map(|kind| (!kind.is_empty()).then_some(kind));
    let jev = JevFilter {
        limit,
        suggest,
        topic,
        brainstorm,
    };
    if !jev.narrows() {
        return Ok(jev);
    }
    if let Some(Some(kind)) = brainstorm
        && !BRAINSTORM_KINDS.contains(&kind)
    {
        return Err(InvalidFilter(format!(
            "--brainstorm takes {}, or nothing for any.",
            BRAINSTORM_KINDS.join(", ")
        )));
    }
    if let Some(suggest) = suggest
        && !SUGGESTIONS.contains(&suggest)
    {
        return Err(InvalidFilter(format!(
            "--suggest must be one of {}.",
            SUGGESTIONS.join(", ")
        )));
    }
    if let Some(topic) = topic
        && !profile.topics.iter().any(|known| known == topic)
    {
        return Err(InvalidFilter(format!(
            "--topic must be one of {}.",
            profile.topics.join(", ")
        )));
    }
    Ok(jev)
}

impl JevFilter<'_> {
    /// Whether any Jev filter is set; without one, unjudged chats count.
    fn narrows(&self) -> bool {
        self.suggest.is_some() || self.topic.is_some() || self.brainstorm.is_some()
    }

    /// The second half of `applyJevFiltersAndLimit`: unjudged chats never
    /// match a Jev filter, and `--limit` counts matches after it.
    pub fn apply<'j, T>(
        &self,
        rows: Vec<T>,
        judgment: impl Fn(&T) -> Option<&'j JudgmentRow>,
        profile: &Profile,
    ) -> Result<Vec<T>, PolicyError> {
        let take = |rows: Vec<T>| match self.limit {
            Some(limit) => rows.into_iter().take(limit).collect(),
            None => rows,
        };
        if !self.narrows() {
            return Ok(take(rows));
        }
        let mut kept = Vec::new();
        for row in rows {
            let Some(found) = judgment(&row) else {
                continue;
            };
            let judged = Judged::new(found, profile)?;
            let verdict = judged.verdict()?;
            if self.matches(&verdict, &judged)? {
                kept.push(row);
            }
        }
        Ok(take(kept))
    }

    fn matches(&self, verdict: &Verdict, judged: &Judged<'_>) -> Result<bool, PolicyError> {
        if self
            .suggest
            .is_some_and(|suggest| verdict.suggestion != suggest)
        {
            return Ok(false);
        }
        if let Some(kind) = self.brainstorm {
            let Some(brainstorm) = verdict.brainstorm.as_deref() else {
                return Ok(false);
            };
            if kind.is_some_and(|kind| kind != brainstorm) {
                return Ok(false);
            }
        }
        match self.topic {
            Some(topic) => Ok(judged.topic()? == topic),
            None => Ok(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_735_689_600_000; // 2025-01-01T00:00:00.000Z

    fn filter() -> Filter {
        Filter::default()
    }

    #[test]
    fn ages_and_dates_combine_as_to_filter_does() {
        let chosen = selection(
            &Filter {
                older_than: Some("30d".into()),
                before: Some("2024-11-01".into()),
                newer_than: Some("1y".into()),
                after: Some("2024-06-01".into()),
                ..filter()
            },
            NOW,
        )
        .expect("valid");
        assert_eq!(
            chosen.index.updated_before.as_deref(),
            Some("2024-11-01T00:00:00.000Z"),
            "the earlier bound wins"
        );
        assert_eq!(
            chosen.index.updated_after.as_deref(),
            Some("2024-06-01T00:00:00.000Z"),
            "the later bound wins"
        );
        assert_eq!(chosen.index.archived, Some(false));
        let all = selection(
            &Filter {
                all: true,
                archived: true,
                ..filter()
            },
            NOW,
        )
        .expect("valid");
        assert_eq!(all.index.archived, None);
    }

    #[test]
    fn bad_values_get_the_ts_clis_messages() {
        let error = |f: Filter| selection(&f, NOW).err().map(|e| e.0);
        assert_eq!(
            error(Filter {
                older_than: Some("3x".into()),
                ..filter()
            })
            .as_deref(),
            Some("Invalid age \"3x\". Use a number and unit, e.g. 30d, 12w, 6m, 2y.")
        );
        assert_eq!(
            error(Filter {
                before: Some("soon".into()),
                ..filter()
            })
            .as_deref(),
            Some("Invalid date \"soon\". Use YYYY-MM-DD.")
        );
        let profile = Profile::builtin();
        let jev_error = |f: Filter| jev_filter(&f, &profile).err().map(|e| e.0);
        assert_eq!(
            jev_error(Filter {
                limit: Some("0".into()),
                ..filter()
            })
            .as_deref(),
            Some("--limit must be a positive whole number.")
        );
        assert_eq!(
            jev_error(Filter {
                suggest: Some("burn".into()),
                ..filter()
            })
            .as_deref(),
            Some("--suggest must be one of delete, archive, keep.")
        );
        assert_eq!(
            jev_error(Filter {
                brainstorm: Some("poem".into()),
                ..filter()
            })
            .as_deref(),
            Some("--brainstorm takes writing, sermon, product, other, or nothing for any.")
        );
        assert!(
            jev_error(Filter {
                topic: Some("cooking".into()),
                ..filter()
            })
            .is_some_and(|message| message.starts_with("--topic must be one of employer_work, "))
        );
        assert!(
            jev_error(Filter {
                limit: Some("1e1".into()),
                ..filter()
            })
            .is_none()
        );
    }

    #[test]
    fn the_title_regex_is_case_insensitive_and_reads_both_titles() {
        let chosen = selection(
            &Filter {
                title: Some("^acc(?=urate)".into()),
                ..filter()
            },
            NOW,
        )
        .expect("valid");
        let chat = IndexedConversation {
            id: "x".into(),
            title: "Original".into(),
            create_time: String::new(),
            update_time: String::new(),
            is_archived: false,
            pinned: false,
            project_id: None,
            local_title: Some("Accurate".into()),
        };
        assert!(title_matches(chosen.title.as_ref(), &chat));
        let original = selection(
            &Filter {
                title: Some("ORIG".into()),
                ..filter()
            },
            NOW,
        )
        .expect("valid");
        assert!(title_matches(original.title.as_ref(), &chat));
    }
}
