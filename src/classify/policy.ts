import type { Judgment } from "../index/classification-store.ts";
import type { Answers, Topic } from "./questions.ts";
import { DEEP_QUESTIONS_VERSION, type DeepAnswers } from "./deep-questions.ts";
import { refineVerdict } from "./deep-policy.ts";
import { LUNA_JUDGMENT_VERSION } from "./luna-version.ts";

export type Suggestion = "delete" | "archive" | "keep";

export type BrainstormKind = "writing" | "sermon" | "product" | "other";

export type Verdict = { suggestion: Suggestion; unsure: boolean; reason: string; brainstorm: BrainstormKind | null; deep?: true; luna?: true };

// Starting thresholds, to be tuned against the user's review decisions. Jev returns
// raw probabilities; the policy lives here so it can change without re-asking.
const DELETE_MIN_NOTHING = 0.8;
const GUARD_MAX = 0.3; // unfinished / personal must be below this to suggest delete
const ARCHIVE_MAX_WORTH = 1.5;
const BRAINSTORM_MIN = 0.6;
const PRODUCT_IDEA_MIN = 0.7;
// Quick help the user could ask again for can go, even with follow-ups.
const RE_ASKABLE_MIN = 0.7;
const RE_ASKABLE_MAX_PERSONAL = 0.5;
const UNSURE_BAND = [0.35, 0.65] as const;

const inBand = (p: number) => p > UNSURE_BAND[0] && p < UNSURE_BAND[1];

export function decide(a: Answers): Verdict {
	const worth = a.worth_keeping.score;
	const nothing = a.nothing_there.noul;
	const unfinished = a.unfinished.noul;
	const personal = a.personal_record.noul;
	const brainstorming = a.brainstorming.noul;
	const target = a.brainstorm_for.choice;
	const productIdea = a.product_idea?.noul ?? 0;
	const timeBound = a.time_bound?.noul ?? 0;
	const overtaken = a.overtaken_by_time?.noul ?? 0;
	// Brainstorms seed articles and apps the user comes back to, so they're kept.
	const brainstorm: BrainstormKind | null = brainstorming >= BRAINSTORM_MIN &&
		(target !== "product" || (productIdea >= PRODUCT_IDEA_MIN && a.topic?.choice !== "employer_work"))
		? (target === "none" ? "other" : target) : null;
	const reAskable = a.re_askable.noul;
	const reason =
		`nothing ${nothing.toFixed(2)} · re-askable ${reAskable.toFixed(2)} · worth ${worth.toFixed(1)}/3 · unfinished ${unfinished.toFixed(2)} · ` +
		`personal ${personal.toFixed(2)} · brainstorm ${brainstorming.toFixed(2)} · product idea ${productIdea.toFixed(2)} · time-bound ${timeBound.toFixed(2)} · overtaken ${overtaken.toFixed(2)}${brainstorm ? ` (${brainstorm})` : ""}`;
	const shaky = inBand(unfinished) || inBand(personal) || inBand(brainstorming) || a.worth_keeping.confidence < 0.4;
	const temporalUncertain = timeBound >= 0.6 && overtaken >= 0.5 && overtaken < 0.8;

	if (brainstorm) return { suggestion: "keep", unsure: inBand(brainstorming), reason, brainstorm };
	if (timeBound >= 0.7 && overtaken >= 0.8) {
		const conflict = personal >= 0.6 || unfinished >= 0.6 || worth >= 2;
		return { suggestion: "delete", unsure: conflict, reason: `${reason} · overtaken by time`, brainstorm };
	}
	const guardsClear = unfinished < GUARD_MAX && personal < GUARD_MAX;
	if (nothing >= DELETE_MIN_NOTHING && guardsClear && worth < 1) {
		return { suggestion: "delete", unsure: shaky || temporalUncertain, reason, brainstorm };
	}
	// Unfinished doesn't guard here: an unresolved quick question is still re-askable.
	if (reAskable >= RE_ASKABLE_MIN && personal < RE_ASKABLE_MAX_PERSONAL) {
		return { suggestion: "delete", unsure: inBand(reAskable) || inBand(personal) || temporalUncertain, reason, brainstorm };
	}
	if (worth < ARCHIVE_MAX_WORTH && unfinished < 0.5 && personal < 0.5) {
		return { suggestion: "archive", unsure: shaky || temporalUncertain, reason, brainstorm };
	}
	// A torn re-askable score means the user might well delete it: surface it as keep?.
	return { suggestion: "keep", unsure: inBand(reAskable) || a.worth_keeping.confidence < 0.4 || temporalUncertain, reason, brainstorm };
}

// Whether Jev backs carrying out `action` on a chat: a delete needs a confident
// delete verdict; an archive is fine for anything Jev wouldn't keep.
export function approves(action: "delete" | "archive", v: Verdict): boolean {
	if (v.unsure) return false;
	return action === "delete" ? v.suggestion === "delete" : v.suggestion !== "keep";
}

export function baseVerdictOf(j: Judgment): Verdict {
	return decide(JSON.parse(j.answers) as Answers);
}

export function jevVerdictOf(j: Judgment): Verdict {
	const base = baseVerdictOf(j);
	return base.unsure && j.deep_version === DEEP_QUESTIONS_VERSION && j.deep_answers
		? refineVerdict(base, JSON.parse(j.deep_answers) as DeepAnswers)
		: base;
}

// Product mentions can make Jev overconfident. Review every product label,
// along with borderline candidates, against the actual user decisions.
export function needsProductReview(j: Judgment): boolean {
	const a = JSON.parse(j.answers) as Answers;
	if (topicOf(j) === "employer_work") return false;
	const product = a.product_idea?.noul ?? 0;
	const targetProduct = a.brainstorm_for.choice === "product" && a.brainstorming.noul >= 0.5;
	return jevVerdictOf(j).brainstorm === "product" ||
		(targetProduct && product >= 0.4) ||
		(topicOf(j) === "side_projects" && product >= 0.45 && product < 0.75);
}

function acceptedBrainstorm(j: Judgment, kind: BrainstormKind | null): BrainstormKind | null {
	if (kind !== "product") return kind;
	const a = JSON.parse(j.answers) as Answers;
	return (a.product_idea?.noul ?? 0) >= 0.4 && topicOf(j) !== "employer_work" ? kind : null;
}

export function needsTimeReview(j: Judgment): boolean {
	const a = JSON.parse(j.answers) as Answers;
	const bound = a.time_bound?.noul ?? 0;
	const expired = a.overtaken_by_time?.noul ?? 0;
	if (bound < 0.6 || expired < 0.5) return false;
	return expired < 0.8 || a.personal_record.noul >= 0.6 || a.unfinished.noul >= 0.6 || a.worth_keeping.score >= 2;
}

export function needsTimeRefresh(j: Judgment, asOf: string): boolean {
	const a = JSON.parse(j.answers) as Answers;
	if ((a.time_bound?.noul ?? 0) < 0.7 || (a.overtaken_by_time?.noul ?? 0) >= 0.8) return false;
	const durableEvidence = a.personal_record.noul >= 0.8 && a.worth_keeping.score >= 2;
	if (durableEvidence) return j.classified_at.slice(0, 7) !== asOf.slice(0, 7);
	return Date.parse(asOf) - Date.parse(j.classified_at.slice(0, 10)) >= 7 * 24 * 60 * 60 * 1000;
}

export function verdictOf(j: Judgment): Verdict {
	const jev = jevVerdictOf(j);
	if (j.luna_version !== LUNA_JUDGMENT_VERSION || !j.luna_suggestion) return jev;
	const reason = `${jev.reason} · Luna: ${j.luna_reason ?? "reviewed"}`;
	if (j.luna_suggestion === "delete") {
		const deep = j.deep_answers ? JSON.parse(j.deep_answers) as DeepAnswers : null;
		if (deep && deep.personal_record_lost.noul >= 0.7 && deep.original_thinking_lost.noul >= 0.6) {
			return { ...jev, suggestion: "keep", unsure: false,
				reason: `${reason} · protected personal history and original thinking`, luna: true };
		}
		const a = JSON.parse(j.answers) as Answers;
		const personalRecord = a.personal_record.noul >= 0.6 && a.worth_keeping.score >= 1.5;
		const artifact = (deep?.reusable_artifact_lost.noul ?? 0) >= 0.7 && a.worth_keeping.score >= 2;
		if (!needsTimeReview(j) && (personalRecord || artifact)) {
			return { ...jev, suggestion: jev.brainstorm ? "keep" : "archive", unsure: false,
				reason: `${reason} · protected possible personal record or artifact`, luna: true };
		}
	}
	if (jev.unsure && j.deep_version === DEEP_QUESTIONS_VERSION) {
		return { suggestion: j.luna_suggestion, unsure: false, reason,
			brainstorm: acceptedBrainstorm(j, j.luna_brainstorm ?? null), deep: true, luna: true };
	}
	if (needsTimeReview(j)) {
		const brainstorm = acceptedBrainstorm(j, j.luna_brainstorm ?? null);
		return { suggestion: brainstorm ? "keep" : j.luna_suggestion, unsure: false, reason,
			brainstorm, deep: jev.deep, luna: true };
	}
	if (needsProductReview(j)) {
		const brainstorm = acceptedBrainstorm(j, j.luna_brainstorm ?? null);
		const rank = { delete: 0, archive: 1, keep: 2 } as const;
		const suggestion = brainstorm ? "keep" : rank[j.luna_suggestion] > rank[jev.suggestion] ? j.luna_suggestion : jev.suggestion;
		return { ...jev, suggestion, brainstorm, reason, luna: true };
	}
	return jev;
}

export function topicOf(j: Judgment): Topic {
	return (j.topic ?? (JSON.parse(j.answers) as Answers).topic.choice) as Topic;
}
