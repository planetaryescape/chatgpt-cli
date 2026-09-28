import { expect, test } from "bun:test";
import type { Judgment } from "../index/classification-store.ts";
import { DEEP_QUESTIONS_VERSION } from "./deep-questions.ts";
import { LUNA_JUDGMENT_VERSION } from "./luna-version.ts";
import { baseVerdictOf, needsProductReview, needsTimeRefresh, needsTimeReview, verdictOf } from "./policy.ts";

function judgment(worthConfidence: number): Judgment {
	return {
		id: "chat", update_time: "2024-01-01", version: "test", content_kind: "full", classified_at: "2024-01-01",
		answers: JSON.stringify({
			worth_keeping: { score: 1.7, confidence: worthConfidence }, nothing_there: { noul: 0.1 },
			unfinished: { noul: 0.1 }, personal_record: { noul: 0.1 }, re_askable: { noul: 0.5 },
			brainstorming: { noul: 0.1 }, brainstorm_for: { choice: "none" }, topic: { choice: "other" },
		}),
		deep_version: DEEP_QUESTIONS_VERSION,
		deep_answers: JSON.stringify({
			personal_record_lost: { noul: 0.3 }, reusable_artifact_lost: { noul: 0.3 },
			original_thinking_lost: { noul: 0.3 }, work_to_resume: { noul: 0.3 }, creative_idea_lost: { noul: 0.3 },
			reaskable_without_loss: { noul: 0.5 }, worth_finding_again: { score: 1.7, confidence: 0.3 },
		}),
	};
}

test("current Luna review resolves a Jev unsure verdict", () => {
	const j = { ...judgment(0.3), luna_version: LUNA_JUDGMENT_VERSION, luna_suggestion: "archive" as const,
		luna_brainstorm: null, luna_reason: "Retain for reference" };
	expect(verdictOf(j)).toMatchObject({ suggestion: "archive", unsure: false, luna: true });
	expect(verdictOf({ ...j, luna_version: LUNA_JUDGMENT_VERSION - 1 }).unsure).toBe(true);
});

test("Luna cannot override an already sure Jev verdict", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.re_askable.noul = 0.1;
	const j = { ...base, answers: JSON.stringify(answers), luna_version: LUNA_JUDGMENT_VERSION, luna_suggestion: "delete" as const,
		luna_brainstorm: null, luna_reason: "wrong" };
	expect(verdictOf(j)).toMatchObject({ suggestion: "keep", unsure: false });
});

test("a conflicting product brainstorm gets a deeper review that can remove its label", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.re_askable.noul = 0.1;
	answers.brainstorming.noul = 0.9;
	answers.brainstorm_for.choice = "product";
	answers.product_idea = { noul: 0.75 };
	answers.topic.choice = "coding_general";
	const j = { ...base, answers: JSON.stringify(answers) };
	expect(needsProductReview(j)).toBe(true);
	expect(verdictOf(j).brainstorm).toBe("product");
	expect(verdictOf({ ...j, luna_version: LUNA_JUDGMENT_VERSION, luna_suggestion: "keep", luna_brainstorm: null,
		luna_reason: "Generic technical exercise" })).toMatchObject({ brainstorm: null, luna: true });
	answers.product_idea.noul = 0.2;
	expect(needsProductReview({ ...j, answers: JSON.stringify(answers) })).toBe(false);
});

test("a high-confidence product label still gets a content review", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.brainstorming.noul = 0.93;
	answers.brainstorm_for.choice = "product";
	answers.product_idea = { noul: 0.94 };
	answers.topic.choice = "side_projects";
	const j = { ...base, answers: JSON.stringify(answers) };
	expect(needsProductReview(j)).toBe(true);
	expect(verdictOf({ ...j, luna_version: LUNA_JUDGMENT_VERSION, luna_suggestion: "keep", luna_brainstorm: null,
		luna_reason: "Converted an existing specification" }).brainstorm).toBeNull();
});

test("removing a product label does not discard Luna's safer archive judgment", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.re_askable.noul = 0.9;
	answers.brainstorming.noul = 0.61;
	answers.brainstorm_for.choice = "product";
	answers.product_idea = { noul: 0.5 };
	answers.topic.choice = "side_projects";
	const j = { ...base, answers: JSON.stringify(answers), luna_version: LUNA_JUDGMENT_VERSION,
		luna_suggestion: "archive" as const, luna_brainstorm: null, luna_reason: "Useful implementation work" };
	expect(verdictOf(j)).toMatchObject({ suggestion: "archive", brainstorm: null, unsure: false });
});

test("a finished one-off need is deletable despite low lasting value", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.time_bound = { noul: 0.96 };
	answers.overtaken_by_time = { noul: 0.95 };
	answers.re_askable.noul = 0.25;
	const j = { ...base, answers: JSON.stringify(answers) };
	expect(baseVerdictOf(j)).toMatchObject({ suggestion: "delete", unsure: false });
	expect(needsTimeReview(j)).toBe(false);
	expect(needsTimeRefresh(j, "2026-09-28")).toBe(false);
});

test("a past event with conflicting personal-record evidence goes to Luna", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.time_bound = { noul: 0.97 };
	answers.overtaken_by_time = { noul: 0.93 };
	answers.personal_record.noul = 0.9;
	answers.worth_keeping.score = 2.6;
	const j = { ...base, answers: JSON.stringify(answers) };
	expect(baseVerdictOf(j)).toMatchObject({ suggestion: "delete", unsure: true });
	expect(needsTimeReview(j)).toBe(true);
	expect(verdictOf({ ...j, luna_version: LUNA_JUDGMENT_VERSION, luna_suggestion: "delete", luna_brainstorm: null,
		luna_reason: "Only past flight logistics" })).toMatchObject({ suggestion: "delete", unsure: false, luna: true });
});

test("a time-bound chat is rechecked weekly until its purpose expires", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.time_bound = { noul: 0.9 };
	answers.overtaken_by_time = { noul: 0.1 };
	const j = { ...base, answers: JSON.stringify(answers) };
	expect(needsTimeRefresh(j, "2024-01-01")).toBe(false);
	expect(needsTimeRefresh(j, "2024-01-02")).toBe(false);
	expect(needsTimeRefresh(j, "2024-01-08")).toBe(true);
	answers.personal_record.noul = 0.9;
	answers.worth_keeping.score = 2.5;
	const durable = { ...j, answers: JSON.stringify(answers) };
	expect(needsTimeRefresh(durable, "2024-01-02")).toBe(false);
	expect(needsTimeRefresh(durable, "2024-02-01")).toBe(true);
});

test("an old creative draft is kept even when its original occasion passed", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.brainstorming.noul = 0.9;
	answers.brainstorm_for.choice = "writing";
	answers.time_bound = { noul: 0.9 };
	answers.overtaken_by_time = { noul: 0.9 };
	expect(baseVerdictOf({ ...base, answers: JSON.stringify(answers) })).toMatchObject({
		suggestion: "keep", brainstorm: "writing",
	});
});

test("a past trip with a substantial family record is protected from a delete review", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.time_bound = { noul: 0.95 };
	answers.overtaken_by_time = { noul: 0.6 };
	base.answers = JSON.stringify(answers);
	const deep = JSON.parse(base.deep_answers!);
	deep.personal_record_lost.noul = 0.79;
	deep.original_thinking_lost.noul = 0.66;
	const j = { ...base, deep_answers: JSON.stringify(deep), luna_version: LUNA_JUDGMENT_VERSION,
		luna_suggestion: "delete" as const, luna_brainstorm: null, luna_reason: "Only old trip logistics" };
	expect(verdictOf(j)).toMatchObject({ suggestion: "keep", unsure: false });
});

test("possible personal records and artifacts are archived instead of deleted", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.personal_record.noul = 0.7;
	answers.worth_keeping.score = 2.2;
	const personal = { ...base, answers: JSON.stringify(answers), luna_version: LUNA_JUDGMENT_VERSION,
		luna_suggestion: "delete" as const, luna_brainstorm: null, luna_reason: "Re-askable" };
	expect(verdictOf(personal)).toMatchObject({ suggestion: "archive", unsure: false, luna: true });
	answers.personal_record.noul = 0.1;
	const deep = JSON.parse(base.deep_answers!);
	deep.reusable_artifact_lost.noul = 0.75;
	expect(verdictOf({ ...personal, answers: JSON.stringify(answers), deep_answers: JSON.stringify(deep) }))
		.toMatchObject({ suggestion: "archive", unsure: false, luna: true });
});

test("a logo refinement without product ideation is not a product brainstorm", () => {
	const base = judgment(0.3);
	const answers = JSON.parse(base.answers);
	answers.brainstorming.noul = 0.3;
	answers.product_idea = { noul: 0.1 };
	answers.topic.choice = "writing_creativity";
	const j = { ...base, answers: JSON.stringify(answers), luna_version: LUNA_JUDGMENT_VERSION,
		luna_suggestion: "keep" as const, luna_brainstorm: "product" as const, luna_reason: "Logo direction" };
	expect(verdictOf(j).brainstorm).toBeNull();
});

test("the user's employer product work does not become user-owned product brainstorming", () => {
	const base = judgment(0.9);
	const answers = JSON.parse(base.answers);
	answers.brainstorming.noul = 0.9;
	answers.brainstorm_for.choice = "product";
	answers.product_idea = { noul: 0.9 };
	answers.topic.choice = "employer_work";
	const j = { ...base, answers: JSON.stringify(answers), luna_version: LUNA_JUDGMENT_VERSION,
		luna_suggestion: "keep" as const, luna_brainstorm: "product" as const, luna_reason: "Audit log concept" };
	expect(baseVerdictOf(j).brainstorm).toBeNull();
	expect(needsProductReview(j)).toBe(false);
	expect(verdictOf(j).brainstorm).toBeNull();
});
