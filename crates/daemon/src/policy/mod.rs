//! Suggestions computed on read from Jev's stored answers, so a threshold
//! change never needs a re-run. Ported from the TS CLI's
//! `src/classify/policy.ts` and `deep-policy.ts` @ 1b8c950; the thresholds
//! and the order of the checks are the TS CLI's, and the tests are its
//! `luna-policy.test.ts` and `deep-policy.test.ts`.

pub mod memory;
pub mod profile;

use serde::Deserialize;

use crate::js::to_fixed;
use chatgpt_store::JudgmentRow;
pub use profile::Profile;

const DELETE_MIN_NOTHING: f64 = 0.8;
const GUARD_MAX: f64 = 0.3;
const ARCHIVE_MAX_WORTH: f64 = 1.5;
const BRAINSTORM_MIN: f64 = 0.6;
const PRODUCT_IDEA_MIN: f64 = 0.7;
const RE_ASKABLE_MIN: f64 = 0.7;
const RE_ASKABLE_MAX_PERSONAL: f64 = 0.5;
const UNSURE_BAND: (f64, f64) = (0.35, 0.65);

fn in_band(p: f64) -> bool {
    p > UNSURE_BAND.0 && p < UNSURE_BAND.1
}

/// A verdict as `verdictOf` returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub suggestion: String,
    pub unsure: bool,
    pub reason: String,
    pub brainstorm: Option<String>,
    pub deep: bool,
    pub luna: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("the stored Jev answers for {id} are unreadable: {why}")]
pub struct PolicyError {
    pub id: String,
    pub why: String,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct Noul {
    noul: Option<f64>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct Score {
    score: Option<f64>,
    confidence: Option<f64>,
}

#[derive(Deserialize, Default, Clone)]
struct Choice {
    choice: Option<String>,
}

/// Jev's answers. Optional where policy.ts reads them with `?.`.
#[derive(Deserialize)]
struct RawAnswers {
    worth_keeping: Option<Score>,
    nothing_there: Option<Noul>,
    unfinished: Option<Noul>,
    personal_record: Option<Noul>,
    brainstorming: Option<Noul>,
    brainstorm_for: Option<Choice>,
    re_askable: Option<Noul>,
    product_idea: Option<Noul>,
    time_bound: Option<Noul>,
    overtaken_by_time: Option<Noul>,
    topic: Option<Choice>,
}

/// The answers with the fields policy.ts needs, resolved: a required one
/// missing is an error, as the TS CLI would throw reading it.
struct Answers {
    worth: f64,
    worth_confidence: f64,
    nothing: f64,
    unfinished: f64,
    personal: f64,
    brainstorming: f64,
    target: String,
    re_askable: f64,
    product_idea: f64,
    time_bound: f64,
    overtaken: f64,
    topic: Option<String>,
}

fn required(value: Option<f64>, name: &str) -> Result<f64, String> {
    value.ok_or_else(|| format!("{name} is missing"))
}

impl Answers {
    fn parse(json: &str) -> Result<Self, String> {
        let raw: RawAnswers = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let noul = |value: Option<Noul>, name: &str| required(value.and_then(|v| v.noul), name);
        let worth = raw.worth_keeping.unwrap_or_default();
        Ok(Self {
            worth: required(worth.score, "worth_keeping.score")?,
            worth_confidence: required(worth.confidence, "worth_keeping.confidence")?,
            nothing: noul(raw.nothing_there, "nothing_there")?,
            unfinished: noul(raw.unfinished, "unfinished")?,
            personal: noul(raw.personal_record, "personal_record")?,
            brainstorming: noul(raw.brainstorming, "brainstorming")?,
            target: raw
                .brainstorm_for
                .and_then(|choice| choice.choice)
                .ok_or("brainstorm_for.choice is missing")?,
            re_askable: noul(raw.re_askable, "re_askable")?,
            product_idea: raw.product_idea.and_then(|v| v.noul).unwrap_or(0.0),
            time_bound: raw.time_bound.and_then(|v| v.noul).unwrap_or(0.0),
            overtaken: raw.overtaken_by_time.and_then(|v| v.noul).unwrap_or(0.0),
            topic: raw.topic.and_then(|topic| topic.choice),
        })
    }
}

#[derive(Deserialize)]
struct RawDeepAnswers {
    personal_record_lost: Option<Noul>,
    reusable_artifact_lost: Option<Noul>,
    original_thinking_lost: Option<Noul>,
    work_to_resume: Option<Noul>,
    creative_idea_lost: Option<Noul>,
    reaskable_without_loss: Option<Noul>,
    worth_finding_again: Option<Score>,
}

struct DeepAnswers {
    personal: f64,
    artifact: f64,
    original: f64,
    resume: f64,
    idea: f64,
    reaskable: f64,
    worth: f64,
    confidence: f64,
}

impl DeepAnswers {
    fn parse(json: &str) -> Result<Self, String> {
        let raw: RawDeepAnswers = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let noul = |value: Option<Noul>, name: &str| required(value.and_then(|v| v.noul), name);
        let worth = raw.worth_finding_again.unwrap_or_default();
        Ok(Self {
            personal: noul(raw.personal_record_lost, "personal_record_lost")?,
            artifact: noul(raw.reusable_artifact_lost, "reusable_artifact_lost")?,
            original: noul(raw.original_thinking_lost, "original_thinking_lost")?,
            resume: noul(raw.work_to_resume, "work_to_resume")?,
            idea: noul(raw.creative_idea_lost, "creative_idea_lost")?,
            reaskable: noul(raw.reaskable_without_loss, "reaskable_without_loss")?,
            worth: required(worth.score, "worth_finding_again.score")?,
            confidence: required(worth.confidence, "worth_finding_again.confidence")?,
        })
    }
}

/// `decide` in policy.ts.
fn decide(a: &Answers, employer_topic: &str) -> Verdict {
    let product_target_ok = a.target != "product"
        || (a.product_idea >= PRODUCT_IDEA_MIN && a.topic.as_deref() != Some(employer_topic));
    let brainstorm = (a.brainstorming >= BRAINSTORM_MIN && product_target_ok).then(|| {
        if a.target == "none" {
            "other".to_owned()
        } else {
            a.target.clone()
        }
    });
    let reason = format!(
        "nothing {} · re-askable {} · worth {}/3 · unfinished {} · personal {} · brainstorm {} · product idea {} · time-bound {} · overtaken {}{}",
        to_fixed(a.nothing, 2),
        to_fixed(a.re_askable, 2),
        to_fixed(a.worth, 1),
        to_fixed(a.unfinished, 2),
        to_fixed(a.personal, 2),
        to_fixed(a.brainstorming, 2),
        to_fixed(a.product_idea, 2),
        to_fixed(a.time_bound, 2),
        to_fixed(a.overtaken, 2),
        brainstorm
            .as_deref()
            .map(|kind| format!(" ({kind})"))
            .unwrap_or_default()
    );
    let shaky = in_band(a.unfinished)
        || in_band(a.personal)
        || in_band(a.brainstorming)
        || a.worth_confidence < 0.4;
    let temporal_uncertain = a.time_bound >= 0.6 && a.overtaken >= 0.5 && a.overtaken < 0.8;
    let verdict =
        |suggestion: &str, unsure: bool, reason: String, brainstorm: Option<String>| Verdict {
            suggestion: suggestion.to_owned(),
            unsure,
            reason,
            brainstorm,
            deep: false,
            luna: false,
        };

    if brainstorm.is_some() {
        return verdict("keep", in_band(a.brainstorming), reason, brainstorm);
    }
    if a.time_bound >= 0.7 && a.overtaken >= 0.8 {
        let conflict = a.personal >= 0.6 || a.unfinished >= 0.6 || a.worth >= 2.0;
        return verdict(
            "delete",
            conflict,
            format!("{reason} · overtaken by time"),
            brainstorm,
        );
    }
    let guards_clear = a.unfinished < GUARD_MAX && a.personal < GUARD_MAX;
    if a.nothing >= DELETE_MIN_NOTHING && guards_clear && a.worth < 1.0 {
        return verdict("delete", shaky || temporal_uncertain, reason, brainstorm);
    }
    if a.re_askable >= RE_ASKABLE_MIN && a.personal < RE_ASKABLE_MAX_PERSONAL {
        let unsure = in_band(a.re_askable) || in_band(a.personal) || temporal_uncertain;
        return verdict("delete", unsure, reason, brainstorm);
    }
    if a.worth < ARCHIVE_MAX_WORTH && a.unfinished < 0.5 && a.personal < 0.5 {
        return verdict("archive", shaky || temporal_uncertain, reason, brainstorm);
    }
    let unsure = in_band(a.re_askable) || a.worth_confidence < 0.4 || temporal_uncertain;
    verdict("keep", unsure, reason, brainstorm)
}

/// `refineVerdict` in deep-policy.ts.
fn refine(base: &Verdict, a: &DeepAnswers) -> Verdict {
    let reason = format!(
        "{} · deep personal {} · artifact {} · original {} · resume {} · idea {} · re-askable {} · worth {}/3",
        base.reason,
        to_fixed(a.personal, 2),
        to_fixed(a.artifact, 2),
        to_fixed(a.original, 2),
        to_fixed(a.resume, 2),
        to_fixed(a.idea, 2),
        to_fixed(a.reaskable, 2),
        to_fixed(a.worth, 1),
    );
    let settled = |suggestion: &str| Verdict {
        suggestion: suggestion.to_owned(),
        unsure: false,
        reason: reason.clone(),
        brainstorm: base.brainstorm.clone(),
        deep: true,
        luna: base.luna,
    };
    let unresolved = Verdict {
        unsure: true,
        reason: reason.clone(),
        deep: true,
        ..base.clone()
    };
    let loss = [a.personal, a.artifact, a.original, a.resume, a.idea]
        .into_iter()
        .fold(f64::NEG_INFINITY, f64::max);

    // Brainstorms are always kept, including a borderline brainstorm in the
    // original pass that the follow-up cannot confidently settle.
    if a.idea >= 0.8 {
        return Verdict {
            brainstorm: Some(base.brainstorm.clone().unwrap_or_else(|| "other".into())),
            ..settled("keep")
        };
    }
    if base.brainstorm.is_some() && a.idea > 0.2 {
        return unresolved;
    }
    if a.personal >= 0.8 || a.artifact >= 0.8 || a.original >= 0.8 || a.resume >= 0.8 {
        return settled("keep");
    }
    if base.brainstorm.is_some() {
        return unresolved;
    }
    if a.reaskable >= 0.9 && loss <= 0.15 && a.worth <= 1.3 && a.confidence >= 0.6 {
        return settled("delete");
    }
    if loss <= 0.25 && a.worth < 1.5 && a.confidence >= 0.6 {
        return settled("archive");
    }
    if a.worth >= 1.9 && a.confidence >= 0.6 && a.reaskable <= 0.35 {
        return settled("keep");
    }
    unresolved
}

/// A judgment with its answers read once, and the versions that decide
/// which follow-ups count.
pub struct Judged<'a> {
    row: &'a JudgmentRow,
    profile: &'a Profile,
    answers: Answers,
}

impl<'a> Judged<'a> {
    /// Read the judgment's answers; unreadable ones are an error, as the TS
    /// CLI would throw reading them.
    pub fn new(row: &'a JudgmentRow, profile: &'a Profile) -> Result<Self, PolicyError> {
        let answers = Answers::parse(&row.answers).map_err(|why| PolicyError {
            id: row.id.clone(),
            why,
        })?;
        Ok(Self {
            row,
            profile,
            answers,
        })
    }

    fn error(&self, why: String) -> PolicyError {
        PolicyError {
            id: self.row.id.clone(),
            why,
        }
    }

    /// `topicOf`: the stored topic, else Jev's answer, else none. The TS
    /// CLI throws on a judgment without `topic.choice`; here it simply has
    /// no topic, so one odd judgment can't fail `list` or `stats`.
    pub fn topic(&self) -> Option<String> {
        self.row
            .topic
            .clone()
            .or_else(|| self.answers.topic.clone())
    }

    fn deep_is_current(&self) -> bool {
        self.row.deep_version.as_deref() == Some(self.profile.deep_questions_version.as_str())
    }

    fn deep_answers(&self) -> Option<&str> {
        self.row
            .deep_answers
            .as_deref()
            .filter(|answers| !answers.is_empty())
    }

    /// `baseVerdictOf`.
    pub fn base_verdict(&self) -> Result<Verdict, PolicyError> {
        Ok(decide(&self.answers, &self.profile.employer_topic))
    }

    /// `jevVerdictOf`.
    fn jev_verdict(&self) -> Result<Verdict, PolicyError> {
        let base = self.base_verdict()?;
        match self.deep_answers() {
            Some(deep) if base.unsure && self.deep_is_current() => {
                let deep = DeepAnswers::parse(deep).map_err(|why| self.error(why))?;
                Ok(refine(&base, &deep))
            }
            _ => Ok(base),
        }
    }

    /// `needsProductReview`.
    pub fn needs_product_review(&self) -> Result<bool, PolicyError> {
        let a = &self.answers;
        if self.topic().as_deref() == Some(self.profile.employer_topic.as_str()) {
            return Ok(false);
        }
        let target_product = a.target == "product" && a.brainstorming >= 0.5;
        Ok(self.jev_verdict()?.brainstorm.as_deref() == Some("product")
            || (target_product && a.product_idea >= 0.4)
            || (self.topic().as_deref() == Some("side_projects")
                && a.product_idea >= 0.45
                && a.product_idea < 0.75))
    }

    /// `acceptedBrainstorm`.
    fn accepted_brainstorm(&self, kind: Option<&str>) -> Result<Option<String>, PolicyError> {
        if kind != Some("product") {
            return Ok(kind.map(str::to_owned));
        }
        let a = &self.answers;
        Ok((a.product_idea >= 0.4
            && self.topic().as_deref() != Some(self.profile.employer_topic.as_str()))
        .then(|| "product".to_owned()))
    }

    /// `needsTimeReview`.
    pub fn needs_time_review(&self) -> Result<bool, PolicyError> {
        let a = &self.answers;
        if a.time_bound < 0.6 || a.overtaken < 0.5 {
            return Ok(false);
        }
        Ok(a.overtaken < 0.8 || a.personal >= 0.6 || a.unfinished >= 0.6 || a.worth >= 2.0)
    }

    /// `verdictOf`: Jev's verdict, settled or adjusted by a current Luna
    /// review.
    pub fn verdict(&self) -> Result<Verdict, PolicyError> {
        let jev = self.jev_verdict()?;
        let luna_current = self.row.luna_version == Some(self.profile.luna_version);
        let luna_suggestion = match self.row.luna_suggestion.as_deref() {
            Some(suggestion) if luna_current && !suggestion.is_empty() => suggestion,
            _ => return Ok(jev),
        };
        let reason = format!(
            "{} · Luna: {}",
            jev.reason,
            self.row.luna_reason.as_deref().unwrap_or("reviewed")
        );
        if luna_suggestion == "delete" {
            // Only these three follow-up answers are read here, whatever the
            // follow-up's version, as in verdictOf.
            let deep: Option<RawDeepAnswers> = self
                .deep_answers()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|error| self.error(error.to_string()))?;
            let deep_noul = |pick: fn(&RawDeepAnswers) -> Option<Noul>| {
                deep.as_ref().and_then(pick).and_then(|answer| answer.noul)
            };
            let personal_lost = deep_noul(|deep| deep.personal_record_lost);
            let original_lost = deep_noul(|deep| deep.original_thinking_lost);
            if personal_lost.is_some_and(|p| p >= 0.7) && original_lost.is_some_and(|o| o >= 0.6) {
                return Ok(Verdict {
                    suggestion: "keep".into(),
                    unsure: false,
                    reason: format!("{reason} · protected personal history and original thinking"),
                    luna: true,
                    ..jev
                });
            }
            let a = &self.answers;
            let personal_record = a.personal >= 0.6 && a.worth >= 1.5;
            let artifact = deep_noul(|deep| deep.reusable_artifact_lost).unwrap_or(0.0) >= 0.7
                && a.worth >= 2.0;
            if !self.needs_time_review()? && (personal_record || artifact) {
                return Ok(Verdict {
                    suggestion: if jev.brainstorm.is_some() {
                        "keep"
                    } else {
                        "archive"
                    }
                    .into(),
                    unsure: false,
                    reason: format!("{reason} · protected possible personal record or artifact"),
                    luna: true,
                    ..jev
                });
            }
        }
        let luna_brainstorm = self.row.luna_brainstorm.as_deref();
        if jev.unsure && self.deep_is_current() {
            return Ok(Verdict {
                suggestion: luna_suggestion.to_owned(),
                unsure: false,
                reason,
                brainstorm: self.accepted_brainstorm(luna_brainstorm)?,
                deep: true,
                luna: true,
            });
        }
        if self.needs_time_review()? {
            let brainstorm = self.accepted_brainstorm(luna_brainstorm)?;
            return Ok(Verdict {
                suggestion: if brainstorm.is_some() {
                    "keep".to_owned()
                } else {
                    luna_suggestion.to_owned()
                },
                unsure: false,
                reason,
                brainstorm,
                deep: jev.deep,
                luna: true,
            });
        }
        if self.needs_product_review()? {
            let brainstorm = self.accepted_brainstorm(luna_brainstorm)?;
            let rank = |suggestion: &str| match suggestion {
                "delete" => Some(0),
                "archive" => Some(1),
                "keep" => Some(2),
                _ => None,
            };
            let suggestion = if brainstorm.is_some() {
                "keep".to_owned()
            } else if rank(luna_suggestion) > rank(&jev.suggestion) {
                luna_suggestion.to_owned()
            } else {
                jev.suggestion.clone()
            };
            return Ok(Verdict {
                suggestion,
                brainstorm,
                reason,
                luna: true,
                ..jev
            });
        }
        Ok(jev)
    }
}

#[cfg(test)]
mod tests;
