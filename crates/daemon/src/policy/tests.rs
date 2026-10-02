//! Ported from the TS CLI's src/classify/luna-policy.test.ts and
//! deep-policy.test.ts @ 1b8c950, one test per TS test.

#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};

use super::*;

fn profile() -> Profile {
    Profile::builtin()
}

fn base_answers(worth_confidence: f64) -> Value {
    json!({
        "worth_keeping": { "score": 1.7, "confidence": worth_confidence }, "nothing_there": { "noul": 0.1 },
        "unfinished": { "noul": 0.1 }, "personal_record": { "noul": 0.1 }, "re_askable": { "noul": 0.5 },
        "brainstorming": { "noul": 0.1 }, "brainstorm_for": { "choice": "none" }, "topic": { "choice": "other" },
    })
}

fn base_deep() -> Value {
    json!({
        "personal_record_lost": { "noul": 0.3 }, "reusable_artifact_lost": { "noul": 0.3 },
        "original_thinking_lost": { "noul": 0.3 }, "work_to_resume": { "noul": 0.3 }, "creative_idea_lost": { "noul": 0.3 },
        "reaskable_without_loss": { "noul": 0.5 }, "worth_finding_again": { "score": 1.7, "confidence": 0.3 },
    })
}

fn row(answers: &Value) -> JudgmentRow {
    JudgmentRow {
        id: "chat".into(),
        update_time: "2024-01-01".into(),
        version: "test".into(),
        content_kind: "full".into(),
        answers: answers.to_string(),
        classified_at: "2024-01-01".into(),
        topic: None,
        deep_answers: Some(base_deep().to_string()),
        deep_version: Some(profile().deep_questions_version),
        ..JudgmentRow::default()
    }
}

fn with_luna(
    mut row: JudgmentRow,
    suggestion: &str,
    brainstorm: Option<&str>,
    reason: &str,
) -> JudgmentRow {
    row.luna_version = Some(profile().luna_version);
    row.luna_suggestion = Some(suggestion.into());
    row.luna_brainstorm = brainstorm.map(Into::into);
    row.luna_reason = Some(reason.into());
    row
}

fn verdict(row: &JudgmentRow) -> Verdict {
    Judged::new(row, &profile()).unwrap().verdict().unwrap()
}

fn base_verdict(row: &JudgmentRow) -> Verdict {
    Judged::new(row, &profile())
        .unwrap()
        .base_verdict()
        .unwrap()
}

fn judged(row: &JudgmentRow) -> bool {
    Judged::new(row, &profile())
        .unwrap()
        .needs_product_review()
        .unwrap()
}

fn time_review(row: &JudgmentRow) -> bool {
    Judged::new(row, &profile())
        .unwrap()
        .needs_time_review()
        .unwrap()
}

#[test]
fn current_luna_review_resolves_a_jev_unsure_verdict() {
    let j = with_luna(
        row(&base_answers(0.3)),
        "archive",
        None,
        "Retain for reference",
    );
    let v = verdict(&j);
    assert_eq!(
        (v.suggestion.as_str(), v.unsure, v.luna),
        ("archive", false, true)
    );
    let mut old = j.clone();
    old.luna_version = Some(profile().luna_version - 1);
    assert!(verdict(&old).unsure);
}

#[test]
fn luna_cannot_override_an_already_sure_jev_verdict() {
    let mut answers = base_answers(0.9);
    answers["re_askable"]["noul"] = json!(0.1);
    let j = with_luna(row(&answers), "delete", None, "wrong");
    let v = verdict(&j);
    assert_eq!((v.suggestion.as_str(), v.unsure), ("keep", false));
}

#[test]
fn a_conflicting_product_brainstorm_gets_a_deeper_review_that_can_remove_its_label() {
    let mut answers = base_answers(0.9);
    answers["re_askable"]["noul"] = json!(0.1);
    answers["brainstorming"]["noul"] = json!(0.9);
    answers["brainstorm_for"]["choice"] = json!("product");
    answers["product_idea"] = json!({ "noul": 0.75 });
    answers["topic"]["choice"] = json!("coding_general");
    let j = row(&answers);
    assert!(judged(&j));
    assert_eq!(verdict(&j).brainstorm.as_deref(), Some("product"));
    let reviewed = with_luna(j, "keep", None, "Generic technical exercise");
    let v = verdict(&reviewed);
    assert_eq!((v.brainstorm, v.luna), (None, true));
    answers["product_idea"]["noul"] = json!(0.2);
    assert!(!judged(&row(&answers)));
}

#[test]
fn a_high_confidence_product_label_still_gets_a_content_review() {
    let mut answers = base_answers(0.9);
    answers["brainstorming"]["noul"] = json!(0.93);
    answers["brainstorm_for"]["choice"] = json!("product");
    answers["product_idea"] = json!({ "noul": 0.94 });
    answers["topic"]["choice"] = json!("side_projects");
    let j = row(&answers);
    assert!(judged(&j));
    let reviewed = with_luna(j, "keep", None, "Converted an existing specification");
    assert_eq!(verdict(&reviewed).brainstorm, None);
}

#[test]
fn removing_a_product_label_keeps_lunas_safer_archive_judgment() {
    let mut answers = base_answers(0.9);
    answers["re_askable"]["noul"] = json!(0.9);
    answers["brainstorming"]["noul"] = json!(0.61);
    answers["brainstorm_for"]["choice"] = json!("product");
    answers["product_idea"] = json!({ "noul": 0.5 });
    answers["topic"]["choice"] = json!("side_projects");
    let j = with_luna(row(&answers), "archive", None, "Useful implementation work");
    let v = verdict(&j);
    assert_eq!(
        (v.suggestion.as_str(), v.brainstorm, v.unsure),
        ("archive", None, false)
    );
}

#[test]
fn a_finished_one_off_need_is_deletable_despite_low_lasting_value() {
    let mut answers = base_answers(0.9);
    answers["time_bound"] = json!({ "noul": 0.96 });
    answers["overtaken_by_time"] = json!({ "noul": 0.95 });
    answers["re_askable"]["noul"] = json!(0.25);
    let j = row(&answers);
    let v = base_verdict(&j);
    assert_eq!((v.suggestion.as_str(), v.unsure), ("delete", false));
    assert!(!time_review(&j));
}

#[test]
fn a_past_event_with_conflicting_personal_record_evidence_goes_to_luna() {
    let mut answers = base_answers(0.9);
    answers["time_bound"] = json!({ "noul": 0.97 });
    answers["overtaken_by_time"] = json!({ "noul": 0.93 });
    answers["personal_record"]["noul"] = json!(0.9);
    answers["worth_keeping"]["score"] = json!(2.6);
    let j = row(&answers);
    let v = base_verdict(&j);
    assert_eq!((v.suggestion.as_str(), v.unsure), ("delete", true));
    assert!(time_review(&j));
    let reviewed = with_luna(j, "delete", None, "Only past flight logistics");
    let v = verdict(&reviewed);
    assert_eq!(
        (v.suggestion.as_str(), v.unsure, v.luna),
        ("delete", false, true)
    );
}

#[test]
fn an_old_creative_draft_is_kept_even_when_its_occasion_passed() {
    let mut answers = base_answers(0.9);
    answers["brainstorming"]["noul"] = json!(0.9);
    answers["brainstorm_for"]["choice"] = json!("writing");
    answers["time_bound"] = json!({ "noul": 0.9 });
    answers["overtaken_by_time"] = json!({ "noul": 0.9 });
    let v = base_verdict(&row(&answers));
    assert_eq!(
        (v.suggestion.as_str(), v.brainstorm.as_deref()),
        ("keep", Some("writing"))
    );
}

#[test]
fn a_past_trip_with_a_substantial_family_record_is_protected_from_a_delete_review() {
    let mut answers = base_answers(0.9);
    answers["time_bound"] = json!({ "noul": 0.95 });
    answers["overtaken_by_time"] = json!({ "noul": 0.6 });
    let mut deep = base_deep();
    deep["personal_record_lost"]["noul"] = json!(0.79);
    deep["original_thinking_lost"]["noul"] = json!(0.66);
    let mut j = with_luna(row(&answers), "delete", None, "Only old trip logistics");
    j.deep_answers = Some(deep.to_string());
    let v = verdict(&j);
    assert_eq!((v.suggestion.as_str(), v.unsure), ("keep", false));
}

#[test]
fn possible_personal_records_and_artifacts_are_archived_instead_of_deleted() {
    let mut answers = base_answers(0.9);
    answers["personal_record"]["noul"] = json!(0.7);
    answers["worth_keeping"]["score"] = json!(2.2);
    let personal = with_luna(row(&answers), "delete", None, "Re-askable");
    let v = verdict(&personal);
    assert_eq!(
        (v.suggestion.as_str(), v.unsure, v.luna),
        ("archive", false, true)
    );
    answers["personal_record"]["noul"] = json!(0.1);
    let mut deep = base_deep();
    deep["reusable_artifact_lost"]["noul"] = json!(0.75);
    let mut artifact = personal.clone();
    artifact.answers = answers.to_string();
    artifact.deep_answers = Some(deep.to_string());
    let v = verdict(&artifact);
    assert_eq!(
        (v.suggestion.as_str(), v.unsure, v.luna),
        ("archive", false, true)
    );
}

#[test]
fn a_logo_refinement_without_product_ideation_is_not_a_product_brainstorm() {
    let mut answers = base_answers(0.3);
    answers["brainstorming"]["noul"] = json!(0.3);
    answers["product_idea"] = json!({ "noul": 0.1 });
    answers["topic"]["choice"] = json!("writing_creativity");
    let j = with_luna(row(&answers), "keep", Some("product"), "Logo direction");
    assert_eq!(verdict(&j).brainstorm, None);
}

#[test]
fn the_users_employer_product_work_is_not_a_user_owned_product_brainstorm() {
    let mut answers = base_answers(0.9);
    answers["brainstorming"]["noul"] = json!(0.9);
    answers["brainstorm_for"]["choice"] = json!("product");
    answers["product_idea"] = json!({ "noul": 0.9 });
    answers["topic"]["choice"] = json!("employer_work");
    let j = with_luna(row(&answers), "keep", Some("product"), "Audit log concept");
    assert_eq!(base_verdict(&j).brainstorm, None);
    assert!(!judged(&j));
    assert_eq!(verdict(&j).brainstorm, None);
}

#[test]
fn the_employer_topic_comes_from_the_profile() {
    let mut answers = base_answers(0.9);
    answers["brainstorming"]["noul"] = json!(0.9);
    answers["brainstorm_for"]["choice"] = json!("product");
    answers["product_idea"] = json!({ "noul": 0.9 });
    answers["topic"]["choice"] = json!("acme_work");
    let j = row(&answers);
    let renamed = Profile {
        employer_topic: "acme_work".into(),
        ..Profile::builtin()
    };
    let judged = Judged::new(&j, &renamed).unwrap();
    assert_eq!(judged.base_verdict().unwrap().brainstorm, None);
    assert_eq!(base_verdict(&j).brainstorm.as_deref(), Some("product"));
}

#[test]
fn reasons_format_numbers_as_to_fixed_does() {
    let mut answers = base_answers(0.9);
    answers["nothing_there"]["noul"] = json!(0.125);
    answers["worth_keeping"]["score"] = json!(2.25);
    let v = base_verdict(&row(&answers));
    assert!(
        v.reason
            .starts_with("nothing 0.13 · re-askable 0.50 · worth 2.3/3"),
        "{}",
        v.reason
    );
}

// deep-policy.test.ts

fn deep(values: &[(&str, f64)]) -> DeepAnswers {
    let get = |name: &str, default: f64| {
        values
            .iter()
            .find(|(key, _)| *key == name)
            .map_or(default, |(_, value)| *value)
    };
    DeepAnswers {
        personal: get("personal", 0.02),
        artifact: get("artifact", 0.02),
        original: get("original", 0.02),
        resume: get("resume", 0.02),
        idea: get("idea", 0.02),
        reaskable: get("reaskable", 0.98),
        worth: get("worth", 0.8),
        confidence: get("confidence", 0.9),
    }
}

fn unsure_keep(brainstorm: Option<&str>) -> Verdict {
    Verdict {
        suggestion: "keep".into(),
        unsure: true,
        reason: "base scores".into(),
        brainstorm: brainstorm.map(Into::into),
        deep: false,
        luna: false,
    }
}

fn refined(base: &Verdict, values: &[(&str, f64)]) -> (String, bool) {
    let v = refine(base, &deep(values));
    (v.suggestion, v.unsure)
}

#[test]
fn deep_follow_ups_settle_unsure_verdicts_as_the_ts_policy_does() {
    let base = unsure_keep(None);
    assert_eq!(refined(&base, &[]), ("delete".into(), false));
    assert_eq!(
        refined(&base, &[("personal", 0.95)]),
        ("keep".into(), false)
    );
    assert_eq!(
        refined(&base, &[("artifact", 0.92)]),
        ("keep".into(), false)
    );
    assert_eq!(
        refined(&base, &[("original", 0.91), ("artifact", 0.04)]),
        ("keep".into(), false)
    );
    assert_eq!(refined(&base, &[("original", 0.5)]), ("keep".into(), true));
    assert_eq!(
        refined(&base, &[("reaskable", 0.55)]),
        ("archive".into(), false)
    );
    assert_eq!(
        refined(
            &base,
            &[("artifact", 0.5), ("worth", 1.7), ("confidence", 0.3)]
        ),
        ("keep".into(), true)
    );
    let writing = unsure_keep(Some("writing"));
    let kept = refine(&writing, &deep(&[("idea", 0.92)]));
    assert_eq!(
        (
            kept.suggestion.as_str(),
            kept.unsure,
            kept.brainstorm.as_deref()
        ),
        ("keep", false, Some("writing"))
    );
    assert_eq!(refined(&writing, &[("idea", 0.5)]), ("keep".into(), true));
}

// Ported from luna-policy.test.ts "a time-bound chat is rechecked weekly
// until its purpose expires".
#[test]
fn a_current_time_bound_chat_is_rechecked_after_a_week_or_a_month_when_durable() {
    let mut answers = base_answers(0.9);
    answers["time_bound"] = json!({ "noul": 0.9 });
    answers["overtaken_by_time"] = json!({ "noul": 0.1 });
    let j = row(&answers);
    let profile = profile();
    let refresh = |row: &JudgmentRow, as_of: &str| {
        Judged::new(row, &profile)
            .unwrap()
            .needs_time_refresh(as_of)
    };
    assert!(!refresh(&j, "2024-01-01"));
    assert!(!refresh(&j, "2024-01-02"));
    assert!(refresh(&j, "2024-01-08"));
    answers["personal_record"]["noul"] = json!(0.9);
    answers["worth_keeping"]["score"] = json!(2.5);
    let durable = row(&answers);
    assert!(!refresh(&durable, "2024-01-02"));
    assert!(refresh(&durable, "2024-02-01"));
    let mut expired = base_answers(0.9);
    expired["time_bound"] = json!({ "noul": 0.9 });
    expired["overtaken_by_time"] = json!({ "noul": 0.9 });
    assert!(!refresh(&row(&expired), "2030-01-01"));
}
