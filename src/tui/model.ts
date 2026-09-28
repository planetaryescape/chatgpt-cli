import { type BrainstormKind, type Suggestion, topicOf, type Verdict, verdictOf } from "../classify/policy.ts";
import { parseAge } from "../commands/select.ts";
import { QUESTIONS_VERSION, TOPICS, type Topic } from "../classify/questions.ts";
import type { ClassificationStore, Judgment } from "../index/classification-store.ts";
import { displayTitle, type ConversationIndex, type IndexedConversation } from "../index/store.ts";

export type Row = { c: IndexedConversation; judgment: Judgment | null; verdict: Verdict | null; topic: Topic | null };

export type Mark = "archive" | "delete";

export type View = {
	query: string;
	suggestion: Suggestion | "all" | "unjudged" | "unsure";
	topic: Topic | "all";
	archived: boolean;
	// "any" or an --older-than style age such as "6m".
	olderThan: string;
	brainstorm: "off" | "any" | BrainstormKind;
};

export const INITIAL_VIEW: View = { query: "", suggestion: "all", topic: "all", archived: false, olderThan: "any", brainstorm: "off" };

export const AGE_CYCLE: readonly string[] = ["any", "30d", "6m", "1y", "2y", "3y"];
export const BRAINSTORM_CYCLE: readonly View["brainstorm"][] = ["off", "any", "writing", "sermon", "product", "other"];

export const TOPIC_CYCLE: readonly (Topic | "all")[] = ["all", ...(Object.keys(TOPICS) as Topic[])];

export function loadRows(index: ConversationIndex, store: ClassificationStore): Row[] {
	const judgments = store.currentJudgments(QUESTIONS_VERSION);
	return index.query({ includePinned: true }).map((c) => {
		const judgment = judgments.get(c.id) ?? null;
		return { c, judgment, verdict: judgment ? verdictOf(judgment) : null, topic: judgment ? topicOf(judgment) : null };
	});
}

export function visibleRows(rows: Row[], view: View, now = Date.now()): Row[] {
	const q = view.query.trim().toLowerCase();
	const cutoff = view.olderThan === "any" ? null : parseAge(view.olderThan, now).toISOString();
	return rows.filter((r) => {
		if (Boolean(r.c.is_archived) !== view.archived) return false;
		if (cutoff && r.c.update_time >= cutoff) return false;
		if (view.brainstorm !== "off" && (!r.verdict?.brainstorm || (view.brainstorm !== "any" && r.verdict.brainstorm !== view.brainstorm))) return false;
		if (q && !displayTitle(r.c).toLowerCase().includes(q) && !r.c.title.toLowerCase().includes(q)) return false;
		if (view.suggestion === "unjudged" && r.verdict) return false;
		if (view.suggestion === "unsure" && !r.verdict?.unsure) return false;
		if (!["all", "unjudged", "unsure"].includes(view.suggestion) && r.verdict?.suggestion !== view.suggestion) return false;
		return view.topic === "all" || r.topic === view.topic;
	});
}

export function cycle<T>(values: readonly T[], current: T, step: 1 | -1): T {
	const i = values.indexOf(current);
	return values[(i + step + values.length) % values.length] as T;
}

// Keeps the selected row inside the visible window with minimal scrolling.
export function windowStart(selected: number, currentStart: number, height: number, total: number): number {
	if (height <= 0) return 0;
	let start = currentStart;
	if (selected < start) start = selected;
	if (selected >= start + height) start = selected - height + 1;
	return Math.max(0, Math.min(start, Math.max(0, total - height)));
}
