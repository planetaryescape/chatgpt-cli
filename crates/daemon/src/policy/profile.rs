//! The classification versions and topics verdicts are made and read
//! with: this build's. A judgment made at other versions (an older build's,
//! or the retired TS CLI's) is stale until `classify` runs again.

use chatgpt_protocol::ClassificationInfo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub questions_version: String,
    pub deep_questions_version: String,
    pub luna_version: i64,
    pub local_title_version: u32,
    pub memory_version: String,
    pub render_version: u32,
    /// In `stats`' topic table order.
    pub topics: Vec<String>,
    /// The topic policy.ts treats as the user's employer's work.
    pub employer_topic: String,
}

impl Profile {
    /// [`Profile::builtin`], built once: what every verdict is read with.
    pub fn current() -> &'static Self {
        static CURRENT: std::sync::LazyLock<Profile> = std::sync::LazyLock::new(Profile::builtin);
        &CURRENT
    }

    /// This build's versions. The first ones were the TS CLI's @ 1b8c950.
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
        }
    }

    pub fn info(&self) -> ClassificationInfo {
        ClassificationInfo {
            source: "builtin".into(),
            questions_version: self.questions_version.clone(),
            deep_questions_version: self.deep_questions_version.clone(),
            luna_version: u32::try_from(self.luna_version).unwrap_or(0),
            local_title_version: self.local_title_version,
            memory_version: self.memory_version.clone(),
            topics: self.topics.clone(),
            problem: None,
        }
    }
}
