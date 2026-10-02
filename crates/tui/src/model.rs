//! What the list shows: the filters and the window over the rows, ported
//! from the TS CLI's `src/tui/model.ts` @ 1b8c950. Pure, so it's tested
//! without a terminal or a daemon.

use chatgpt_core::js::trim;
use chatgpt_protocol::Row;

/// A pending change to a chat. Marks live only as long as the TUI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Archive,
    Delete,
}

impl Mark {
    /// The mark Jev's suggestion makes (`enter`): none for keep.
    pub fn for_suggestion(suggestion: &str) -> Option<Self> {
        match suggestion {
            "delete" => Some(Self::Delete),
            "archive" => Some(Self::Archive),
            _ => None,
        }
    }
}

/// The suggestion filter, keys `1`–`6`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suggestion {
    All,
    Delete,
    Archive,
    Keep,
    Unjudged,
    Unsure,
}

impl Suggestion {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Delete => "delete",
            Self::Archive => "archive",
            Self::Keep => "keep",
            Self::Unjudged => "unjudged",
            Self::Unsure => "unsure",
        }
    }

    /// `SUGGESTION_KEYS`.
    pub fn for_key(key: char) -> Option<Self> {
        Some(match key {
            '1' => Self::All,
            '2' => Self::Delete,
            '3' => Self::Archive,
            '4' => Self::Keep,
            '5' => Self::Unjudged,
            '6' => Self::Unsure,
            _ => return None,
        })
    }
}

/// Older than: `y` cycles forward, `Y` back.
pub const AGE_CYCLE: [&str; 6] = ["any", "30d", "6m", "1y", "2y", "3y"];
/// Brainstorms: `b` cycles forward, `B` back.
pub const BRAINSTORM_CYCLE: [&str; 6] = ["off", "any", "writing", "sermon", "product", "other"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub query: String,
    pub suggestion: Suggestion,
    /// `all`, or a topic.
    pub topic: String,
    pub archived: bool,
    /// `any`, or an `--older-than` age such as `6m`.
    pub older_than: &'static str,
    /// `off`, `any`, or a brainstorm kind.
    pub brainstorm: &'static str,
}

impl Default for View {
    fn default() -> Self {
        Self {
            query: String::new(),
            suggestion: Suggestion::All,
            topic: "all".to_owned(),
            archived: false,
            older_than: "any",
            brainstorm: "off",
        }
    }
}

/// `parseAge(value, now).toISOString()`: the cutoff an age filter keeps
/// chats updated before.
fn cutoff(age: &str, now_ms: i64) -> Option<String> {
    let (count, unit) = age.split_at(age.len().checked_sub(1)?);
    let days: i64 = match unit {
        "d" => 1,
        "w" => 7,
        "m" => 30,
        "y" => 365,
        _ => return None,
    };
    let millis = now_ms - count.parse::<i64>().ok()? * days * 86_400_000;
    let time = chrono::DateTime::from_timestamp_millis(millis)?;
    Some(time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// `visibleRows`: the indices of the rows `view` shows, in order.
pub fn visible_rows(rows: &[Row], view: &View, now_ms: i64) -> Vec<usize> {
    let query = trim(&view.query).to_lowercase();
    let cutoff = (view.older_than != "any")
        .then(|| cutoff(view.older_than, now_ms))
        .flatten();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| {
            let jev = row.jev.as_ref();
            if (row.is_archived != 0) != view.archived {
                return false;
            }
            if cutoff
                .as_deref()
                .is_some_and(|cutoff| row.update_time.as_str() >= cutoff)
            {
                return false;
            }
            if view.brainstorm != "off" {
                match jev.and_then(|jev| jev.brainstorm.as_deref()) {
                    None => return false,
                    Some(kind) if view.brainstorm != "any" && kind != view.brainstorm => {
                        return false;
                    }
                    Some(_) => {}
                }
            }
            if !query.is_empty()
                && !row.display_title.to_lowercase().contains(&query)
                && !row.title.to_lowercase().contains(&query)
            {
                return false;
            }
            let suggestion_matches = match view.suggestion {
                Suggestion::All => true,
                Suggestion::Unjudged => jev.is_none(),
                Suggestion::Unsure => jev.is_some_and(|jev| jev.unsure),
                wanted => jev.is_some_and(|jev| jev.suggestion == wanted.label()),
            };
            suggestion_matches
                && (view.topic == "all" || row.row_topic.as_ref() == Some(&view.topic))
        })
        .map(|(index, _)| index)
        .collect()
}

/// The value after (or, with `back`, before) `current`, wrapping round.
pub fn cycle<T: PartialEq + Clone>(values: &[T], current: &T, back: bool) -> T {
    let len = values.len();
    let at = values.iter().position(|value| value == current);
    let next = match (at, back) {
        (Some(at), false) => (at + 1) % len,
        (Some(at), true) => (at + len - 1) % len,
        // `indexOf` is -1: `(-1 + 1) % n` and `(-1 - 1 + n) % n`.
        (None, false) => 0,
        (None, true) => len.saturating_sub(2),
    };
    values[next].clone()
}

/// `windowStart`: the first row shown, scrolling only as far as needed to
/// keep the selection visible.
pub fn window_start(selected: usize, current_start: usize, height: usize, total: usize) -> usize {
    if height == 0 {
        return 0;
    }
    let mut start = current_start;
    if selected < start {
        start = selected;
    }
    if selected >= start + height {
        start = selected + 1 - height;
    }
    start.min(total.saturating_sub(height))
}

#[cfg(test)]
pub(crate) mod tests {
    use chatgpt_protocol::Jev;

    use super::*;

    pub fn row(id: &str, title: &str, archived: bool, updated: &str) -> Row {
        Row {
            id: id.to_owned(),
            title: title.to_owned(),
            create_time: updated.to_owned(),
            update_time: updated.to_owned(),
            is_archived: u8::from(archived),
            pinned: 0,
            project_id: None,
            local_title: None,
            display_title: title.to_owned(),
            topic: None,
            row_topic: None,
            jev: None,
        }
    }

    pub fn judged(
        mut row: Row,
        suggestion: &str,
        topic: &str,
        brainstorm: Option<&str>,
        unsure: bool,
    ) -> Row {
        row.topic = Some(topic.to_owned());
        row.row_topic = Some(topic.to_owned());
        row.jev = Some(Jev {
            suggestion: suggestion.to_owned(),
            unsure,
            reason: format!("{suggestion} because"),
            brainstorm: brainstorm.map(str::to_owned),
            deep: false,
            luna: false,
            answers: serde_json::Value::Null,
        });
        row
    }

    fn rows() -> Vec<Row> {
        vec![
            judged(
                row("1", "Sprint names", false, "2024-01-01T00:00:00Z"),
                "delete",
                "health",
                None,
                false,
            ),
            judged(
                row("2", "Devotional draft", false, "2026-09-01T00:00:00Z"),
                "keep",
                "faith",
                Some("sermon"),
                false,
            ),
            judged(
                row("3", "Old archived", true, "2024-01-01T00:00:00Z"),
                "archive",
                "faith",
                None,
                false,
            ),
            row("4", "Unjudged chat", false, "2024-01-01T00:00:00Z"),
        ]
    }

    // 2026-09-27T00:00:00Z, as model.test.ts fixes it.
    const NOW: i64 = 1_790_467_200_000;

    fn ids(view: &View) -> Vec<&'static str> {
        let all = ["1", "2", "3", "4", "5"];
        let rows = rows();
        visible_rows(&rows, view, NOW)
            .into_iter()
            .map(|index| all[index])
            .collect()
    }

    #[test]
    fn archived_chats_show_only_in_the_archived_view() {
        assert_eq!(ids(&View::default()), ["1", "2", "4"]);
        let archived = View {
            archived: true,
            ..View::default()
        };
        assert_eq!(ids(&archived), ["3"]);
    }

    #[test]
    fn title_query_suggestion_and_topic_combine() {
        let view = |patch: fn(&mut View)| {
            let mut view = View::default();
            patch(&mut view);
            view
        };
        assert_eq!(ids(&view(|v| v.query = " DEVOTIONAL ".into())), ["2"]);
        assert_eq!(ids(&view(|v| v.suggestion = Suggestion::Delete)), ["1"]);
        assert_eq!(ids(&view(|v| v.topic = "faith".into())), ["2"]);
        assert_eq!(ids(&view(|v| v.suggestion = Suggestion::Unjudged)), ["4"]);
        let mut torn = rows();
        torn.push(judged(
            row("5", "Rental car options", false, "2024-01-01T00:00:00Z"),
            "keep",
            "health",
            None,
            true,
        ));
        let unsure = view(|v| v.suggestion = Suggestion::Unsure);
        assert_eq!(visible_rows(&torn, &unsure, NOW), [4]);
    }

    #[test]
    fn older_than_keeps_only_chats_updated_before_the_cutoff() {
        let year = View {
            older_than: "1y",
            ..View::default()
        };
        assert_eq!(ids(&year), ["1", "4"]);
        assert_eq!(
            cutoff("30d", NOW).as_deref(),
            Some("2026-08-28T00:00:00.000Z")
        );
        assert_eq!(
            cutoff("6m", NOW).as_deref(),
            Some("2026-03-31T00:00:00.000Z")
        );
    }

    #[test]
    fn brainstorm_filter_matches_any_kind_or_one() {
        let brainstorm = |kind: &'static str| View {
            brainstorm: kind,
            ..View::default()
        };
        assert_eq!(ids(&brainstorm("any")), ["2"]);
        assert_eq!(ids(&brainstorm("sermon")), ["2"]);
        assert!(ids(&brainstorm("writing")).is_empty());
        assert_eq!(cycle(&BRAINSTORM_CYCLE, &"writing", false), "sermon");
    }

    #[test]
    fn window_scrolls_only_as_far_as_needed() {
        assert_eq!(window_start(0, 0, 10, 100), 0);
        assert_eq!(window_start(9, 0, 10, 100), 0);
        assert_eq!(window_start(10, 0, 10, 100), 1);
        assert_eq!(window_start(3, 5, 10, 100), 3);
        assert_eq!(window_start(99, 95, 10, 100), 90);
        assert_eq!(window_start(2, 0, 10, 3), 0);
        assert_eq!(window_start(0, 0, 0, 5), 0);
    }

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle(&["a", "b", "c"], &"c", false), "a");
        assert_eq!(cycle(&["a", "b", "c"], &"a", true), "c");
    }
}
