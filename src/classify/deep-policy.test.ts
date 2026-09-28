import { expect, test } from "bun:test";
import type { DeepAnswers } from "./deep-questions.ts";
import { refineVerdict } from "./deep-policy.ts";
import type { Verdict } from "./policy.ts";

const base: Verdict = { suggestion: "keep", unsure: true, reason: "base scores", brainstorm: null };

function answers(values: { personal?: number; artifact?: number; original?: number; resume?: number; idea?: number; reaskable?: number; worth?: number; confidence?: number } = {}): DeepAnswers {
	return {
		personal_record_lost: { type: "noul", noul: values.personal ?? 0.02 },
		reusable_artifact_lost: { type: "noul", noul: values.artifact ?? 0.02 },
		original_thinking_lost: { type: "noul", noul: values.original ?? 0.02 },
		work_to_resume: { type: "noul", noul: values.resume ?? 0.02 },
		creative_idea_lost: { type: "noul", noul: values.idea ?? 0.02 },
		reaskable_without_loss: { type: "noul", noul: values.reaskable ?? 0.98 },
		worth_finding_again: { type: "score", score: values.worth ?? 0.8, confidence: values.confidence ?? 0.9 },
	} as DeepAnswers;
}

test("confidently re-askable help becomes delete", () => {
	const verdict = refineVerdict(base, answers());
	expect(verdict.suggestion).toBe("delete");
	expect(verdict.unsure).toBe(false);
});

test("a personal record outweighs a contradictory re-askable score", () => {
	const verdict = refineVerdict(base, answers({ personal: 0.95 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(false);
});

test("a reusable artifact becomes keep even when advice could be re-asked", () => {
	const verdict = refineVerdict(base, answers({ artifact: 0.92 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(false);
});

test("original user thinking is protected separately from an assistant artifact", () => {
	const verdict = refineVerdict(base, answers({ original: 0.91, artifact: 0.04 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(false);
});

test("a middling original-thinking score blocks delete", () => {
	const verdict = refineVerdict(base, answers({ original: 0.5 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(true);
});

test("a creative idea remains keep, including a borderline base brainstorm", () => {
	const verdict = refineVerdict({ ...base, brainstorm: "writing" }, answers({ idea: 0.92 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(false);
	expect(verdict.brainstorm).toBe("writing");
});

test("a base brainstorm with inconclusive follow-up stays unsure", () => {
	const verdict = refineVerdict({ ...base, brainstorm: "writing" }, answers({ idea: 0.5 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(true);
});

test("low-value content with no loss but unclear re-askability becomes archive", () => {
	const verdict = refineVerdict(base, answers({ reaskable: 0.55 }));
	expect(verdict.suggestion).toBe("archive");
	expect(verdict.unsure).toBe(false);
});

test("conflicting or low-confidence answers remain unsure", () => {
	const verdict = refineVerdict(base, answers({ artifact: 0.5, worth: 1.7, confidence: 0.3 }));
	expect(verdict.suggestion).toBe("keep");
	expect(verdict.unsure).toBe(true);
});
