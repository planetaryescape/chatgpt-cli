import type { SystemOneResult } from "@typesafe-ai/sdk";

// Bump when a follow-up question or its criteria changes.
export const DEEP_QUESTIONS_VERSION = "2026-09-27.2";

// These ask what deletion would lose, separating personal context from a
// personal record and tailored advice from reusable work or original thinking.
export const DEEP_QUESTIONS = {
	personal_record_lost: {
		type: "noul",
		instructions: "Would deleting this chat lose a specific fact or record about the user's life that they may need later? A passing mention of their child, car, trip, symptom, or circumstances that only frames a generic question does not count.",
		criteria: {
			true: "The chat records a specific personal history, event, measurement, obligation, or decision that cannot be recovered by asking again",
			false: "It only mentions personal context while asking for advice or general information, or contains no personal context",
		},
	},
	reusable_artifact_lost: {
		type: "noul",
		instructions: "Would deleting this chat lose a concrete artifact the user may reuse in its current form: a draft, plan, code, design, analysis of their own data, or a recorded decision? A tailored answer, recommendation, or troubleshooting advice is not an artifact merely because it used the user's preferences or situation.",
		criteria: {
			true: "The chat contains a specific reusable artifact or decision record that would be lost",
			false: "It provides advice, recommendations, explanations, or quick help without a reusable artifact",
		},
	},
	original_thinking_lost: {
		type: "noul",
		instructions: "Did the user develop their own argument, interpretation, analysis, or distinctive line of thought in this conversation that would be difficult to reconstruct from a fresh question? A question, preference, brief personal context, or agreement with the assistant does not count.",
		criteria: {
			true: "The user's own substantive reasoning or perspective is developed here and would be lost",
			false: "The user only asks for help, states a preference or context, or reacts without developing original thinking",
		},
	},
	work_to_resume: {
		type: "noul",
		instructions: "Is there substantive work in this chat that the user was still developing and may return to? A question left unanswered is not work to resume.",
		criteria: {
			true: "A draft, plan, idea, investigation, or task was left in progress",
			false: "The exchange ended, was completed, or never became substantive work",
		},
	},
	creative_idea_lost: {
		type: "noul",
		instructions: "Did the user develop their own idea for a talk, article, essay, app, product, or other thing they want to create? Drafting or outlining counts. Merely requesting examples or an explanation does not.",
		criteria: {
			true: "The user's creative idea, outline, or draft is developed here",
			false: "No user-owned creative idea is developed",
		},
	},
	reaskable_without_loss: {
		type: "noul",
		instructions: "If the user can restate the question and relevant facts from memory, could a fresh answer provide materially the same practical value? Tailored product recommendations, troubleshooting steps, and advice can be re-asked. Answer false if the chat holds hard-to-reconstruct user data, original thinking, a decision, or a reusable artifact.",
		criteria: {
			true: "The useful content is reproducible advice, explanation, recommendations, or momentary help",
			false: "A specific record, contribution, or artifact would be lost",
		},
	},
	worth_finding_again: {
		type: "score",
		instructions: "How valuable would it be for the user to find this exact conversation again? Judge the content that would be lost, not its length or whether the original question was important at the time.",
		criteria: [
			"None: empty, trivial, or no useful answer",
			"Low: useful but reproducible advice, recommendations, or momentary help, even when tailored to the user",
			"Moderate: a specific record, resource, analysis, or artifact whose exact content may be useful to revisit",
			"High: a lasting personal record, decision, plan, draft, original analysis, or creative idea",
		],
	},
} as const;

export type DeepAnswers = SystemOneResult<typeof DEEP_QUESTIONS>["answers"];
