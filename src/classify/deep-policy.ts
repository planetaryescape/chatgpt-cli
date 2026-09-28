import type { Verdict } from "./policy.ts";
import type { DeepAnswers } from "./deep-questions.ts";

export function refineVerdict(base: Verdict, a: DeepAnswers): Verdict {
	const personal = a.personal_record_lost.noul;
	const artifact = a.reusable_artifact_lost.noul;
	const original = a.original_thinking_lost.noul;
	const resume = a.work_to_resume.noul;
	const idea = a.creative_idea_lost.noul;
	const reaskable = a.reaskable_without_loss.noul;
	const worth = a.worth_finding_again.score;
	const confidence = a.worth_finding_again.confidence;
	const reason = `${base.reason} · deep personal ${personal.toFixed(2)} · artifact ${artifact.toFixed(2)} · original ${original.toFixed(2)} · resume ${resume.toFixed(2)} · idea ${idea.toFixed(2)} · re-askable ${reaskable.toFixed(2)} · worth ${worth.toFixed(1)}/3`;
	const unresolved: Verdict = { ...base, unsure: true, reason, deep: true };
	const loss = Math.max(personal, artifact, original, resume, idea);

	// Brainstorms are always kept, including a borderline brainstorm in the
	// original pass that the follow-up cannot confidently settle.
	if (idea >= 0.8) return { ...base, suggestion: "keep", unsure: false, reason, deep: true, brainstorm: base.brainstorm ?? "other" };
	if (base.brainstorm && idea > 0.2) return unresolved;
	if (personal >= 0.8 || artifact >= 0.8 || original >= 0.8 || resume >= 0.8) return { ...base, suggestion: "keep", unsure: false, reason, deep: true };
	if (base.brainstorm) return unresolved;

	if (reaskable >= 0.9 && loss <= 0.15 && worth <= 1.3 && confidence >= 0.6) {
		return { ...base, suggestion: "delete", unsure: false, reason, deep: true };
	}
	if (loss <= 0.25 && worth < 1.5 && confidence >= 0.6) {
		return { ...base, suggestion: "archive", unsure: false, reason, deep: true };
	}
	if (worth >= 1.9 && confidence >= 0.6 && reaskable <= 0.35) {
		return { ...base, suggestion: "keep", unsure: false, reason, deep: true };
	}
	return unresolved;
}
