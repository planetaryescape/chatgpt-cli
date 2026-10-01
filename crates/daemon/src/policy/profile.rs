//! The classification versions and topics verdicts are read with.
//!
//! While the bridge exists the TS CLI writes every judgment, so the versions
//! that decide whether a judgment, follow-up, Luna review or local title is
//! current are the installed TS CLI's, not this build's. They differ in
//! practice: the installed CLI can be a private checkout with its own
//! question versions and topic names. The daemon reads them from the TS
//! sources it bridges to and falls back to the ones built in, which match
//! this repository's `src/` (a test keeps them in step).

use std::path::Path;

use chatgpt_protocol::ClassificationInfo;
use regex::Regex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub questions_version: String,
    pub deep_questions_version: String,
    pub luna_version: i64,
    pub local_title_version: u32,
    pub memory_version: String,
    pub render_version: u32,
    /// In the TS CLI's order, which `stats` uses for its topic table.
    pub topics: Vec<String>,
    /// The topic policy.ts treats as the user's employer's work.
    pub employer_topic: String,
    /// `builtin`, or the TS source directory they came from.
    pub source: String,
    /// Why the TS sources weren't used, when they weren't.
    pub problem: Option<String>,
}

impl Profile {
    /// This repository's TS sources @ 1b8c950.
    pub fn builtin() -> Self {
        Self {
            questions_version: "2026-09-28.9".into(),
            deep_questions_version: "2026-09-27.2".into(),
            luna_version: 8,
            local_title_version: 2,
            memory_version: "2026-09-28.5".into(),
            render_version: 2,
            topics: [
                "employer_work",
                "side_projects",
                "coding_general",
                "writing_creativity",
                "faith",
                "family_relationships",
                "health",
                "home_money_admin",
                "career_employment",
                "travel_transport",
                "learning_culture",
                "other",
            ]
            .map(str::to_owned)
            .to_vec(),
            employer_topic: "employer_work".into(),
            source: "builtin".into(),
            problem: None,
        }
    }

    /// The TS CLI's versions from its sources at `source_dir` (its `src/`),
    /// or the built-in ones with the reason they weren't readable.
    pub fn for_ts_sources(source_dir: Option<&Path>) -> Self {
        let Some(dir) = source_dir else {
            return Self::builtin();
        };
        match read(dir) {
            Ok(profile) => profile,
            Err(problem) => Self {
                problem: Some(format!(
                    "couldn't read the classification versions from {}: {problem}",
                    dir.display()
                )),
                ..Self::builtin()
            },
        }
    }

    pub fn info(&self) -> ClassificationInfo {
        ClassificationInfo {
            source: self.source.clone(),
            questions_version: self.questions_version.clone(),
            deep_questions_version: self.deep_questions_version.clone(),
            luna_version: u32::try_from(self.luna_version).unwrap_or(0),
            local_title_version: self.local_title_version,
            memory_version: self.memory_version.clone(),
            topics: self.topics.clone(),
            problem: self.problem.clone(),
        }
    }
}

fn read(dir: &Path) -> Result<Profile, String> {
    let file = |relative: &str| {
        std::fs::read_to_string(dir.join(relative)).map_err(|error| format!("{relative}: {error}"))
    };
    let questions = file("classify/questions.ts")?;
    let policy = file("classify/policy.ts")?;
    Ok(Profile {
        questions_version: string_constant(&questions, "QUESTIONS_VERSION")?,
        deep_questions_version: string_constant(
            &file("classify/deep-questions.ts")?,
            "DEEP_QUESTIONS_VERSION",
        )?,
        luna_version: number_constant(&file("classify/luna-version.ts")?, "LUNA_JUDGMENT_VERSION")?,
        local_title_version: u32::try_from(number_constant(
            &file("index/store.ts")?,
            "LOCAL_TITLE_VERSION",
        )?)
        .map_err(|_| "LOCAL_TITLE_VERSION is out of range".to_owned())?,
        memory_version: string_constant(
            &file("classify/memories.ts")?,
            "MEMORY_CLASSIFICATION_VERSION",
        )?,
        render_version: u32::try_from(number_constant(
            &file("render/transcript.ts")?,
            "RENDER_VERSION",
        )?)
        .map_err(|_| "RENDER_VERSION is out of range".to_owned())?,
        topics: topics(&questions)?,
        employer_topic: employer_topic(&policy)?,
        source: dir.display().to_string(),
        problem: None,
    })
}

fn capture(source: &str, pattern: &str, what: &str) -> Result<String, String> {
    let pattern = Regex::new(pattern).map_err(|error| error.to_string())?;
    pattern
        .captures(source)
        .and_then(|captures| captures.get(1))
        .map(|found| found.as_str().to_owned())
        .ok_or_else(|| format!("no {what}"))
}

fn string_constant(source: &str, name: &str) -> Result<String, String> {
    capture(
        source,
        &format!(r#"export const {name} = "([^"]+)";"#),
        name,
    )
}

fn number_constant(source: &str, name: &str) -> Result<i64, String> {
    capture(source, &format!(r"export const {name} = (\d+);"), name)?
        .parse()
        .map_err(|_| format!("{name} isn't a whole number"))
}

/// The keys of `export const TOPICS = { … }`, in order.
fn topics(questions: &str) -> Result<Vec<String>, String> {
    let start = questions
        .find("export const TOPICS = {")
        .ok_or("no TOPICS")?;
    let body = &questions[start..];
    let end = body.find("\n}").ok_or("TOPICS never closes")?;
    let key = Regex::new(r"(?m)^\s+([a-z_]+):").map_err(|error| error.to_string())?;
    let topics: Vec<String> = key
        .captures_iter(&body[..end])
        .filter_map(|captures| captures.get(1))
        .map(|found| found.as_str().to_owned())
        .collect();
    if topics.is_empty() {
        return Err("TOPICS is empty".into());
    }
    Ok(topics)
}

/// The topic policy.ts compares with in `needsProductReview`, which every
/// employer check in it uses.
fn employer_topic(policy: &str) -> Result<String, String> {
    let pattern = Regex::new(r#"topicOf\(j\) [!=]== "([a-z_]+)""#).map_err(|e| e.to_string())?;
    let mut found: Vec<&str> = pattern
        .captures_iter(policy)
        .filter_map(|captures| captures.get(1))
        .map(|topic| topic.as_str())
        .filter(|topic| *topic != "side_projects")
        .collect();
    found.dedup();
    match found.as_slice() {
        [topic] => Ok((*topic).to_owned()),
        [] => Err("no employer topic in policy.ts".into()),
        _ => Err(format!(
            "policy.ts names several employer topics: {found:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in versions are this repository's TS sources: a change to
    /// either side fails here.
    #[test]
    fn the_builtin_profile_matches_this_repositorys_ts_sources() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src");
        let read = Profile::for_ts_sources(Some(&src));
        assert_eq!(read.problem, None);
        assert_eq!(
            Profile {
                source: "builtin".into(),
                ..read
            },
            Profile::builtin()
        );
    }

    #[test]
    fn unreadable_sources_fall_back_to_the_builtin_versions_and_say_why() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = Profile::for_ts_sources(Some(dir.path()));
        assert_eq!(
            profile.questions_version,
            Profile::builtin().questions_version
        );
        assert!(profile.problem.is_some_and(|p| p.contains("questions.ts")));
    }

    #[test]
    fn a_renamed_employer_topic_is_read_from_policy() {
        let policy = r#"
            if (topicOf(j) === "acme_work") return false;
            (topicOf(j) === "side_projects" && product >= 0.45)
            return x && topicOf(j) !== "acme_work" ? kind : null;"#;
        assert_eq!(employer_topic(policy).as_deref(), Ok("acme_work"));
    }
}
