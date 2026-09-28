import { type Suggestion, topicOf, verdictOf } from "../classify/policy.ts";
import { QUESTIONS_VERSION, type Topic, TOPICS } from "../classify/questions.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import type { IndexedConversation } from "../index/store.ts";
import type { MemoryCounts } from "../classify/memories.ts";

type Label = Suggestion | `${Suggestion}?`;
const LABELS: Label[] = ["delete", "delete?", "archive", "archive?", "keep", "keep?"];
const emptyCounts = (): Record<Label, number> => ({ delete: 0, "delete?": 0, archive: 0, "archive?": 0, keep: 0, "keep?": 0 });

function table(headers: string[], rows: (string | number)[][]): string {
	const cells = [headers, ...rows.map((r) => r.map(String))];
	const widths = headers.map((_, i) => Math.max(...cells.map((r) => (r[i] ?? "").length)));
	const line = (r: string[]) => r.map((c, i) => (i === 0 ? c.padEnd(widths[i] ?? 0) : c.padStart(widths[i] ?? 0))).join("   ");
	return [line(headers), line(widths.map((w) => "─".repeat(w))), ...cells.slice(1).map(line)].join("\n");
}

export function printStats(rows: IndexedConversation[], store: ClassificationStore) {
	const judgments = store.currentJudgments(QUESTIONS_VERSION);
	const bySuggestion = emptyCounts();
	const byTopic = new Map<Topic, Record<Label, number>>();
	const brainstorms = { writing: 0, sermon: 0, product: 0, other: 0 };
	let unjudged = 0;

	for (const r of rows) {
		const j = judgments.get(r.id);
		if (!j) {
			unjudged++;
			continue;
		}
		const v = verdictOf(j);
		const label: Label = v.unsure ? `${v.suggestion}?` : v.suggestion;
		bySuggestion[label]++;
		if (v.brainstorm) brainstorms[v.brainstorm]++;
		const topic = topicOf(j);
		const t = byTopic.get(topic) ?? emptyCounts();
		t[label]++;
		byTopic.set(topic, t);
	}

	const judged = rows.length - unjudged;
	console.log(`${rows.length} chat(s), ${judged} judged, ${unjudged} not yet judged\n`);
	console.log(table(["suggestion", "chats"], [...LABELS.map((label) => [label, bySuggestion[label]]), ["not judged", unjudged]]));
	console.log(`\n${table(["brainstorm chats (kept)", "chats"], [...Object.entries(brainstorms), ["total", Object.values(brainstorms).reduce((sum, count) => sum + count, 0)]])}`);
	console.log("Counts are conversations, not distinct products or writing pieces.");
	const topics = (Object.keys(TOPICS) as Topic[])
		.map((t) => ({ t, c: byTopic.get(t) ?? emptyCounts() }))
		.sort((a, b) => Object.values(b.c).reduce((sum, n) => sum + n, 0) - Object.values(a.c).reduce((sum, n) => sum + n, 0));
	console.log(`\n${table(["topic", ...LABELS], topics.map(({ t, c }) => [t, ...LABELS.map((label) => c[label])]))}`);
}

export function printMemoryStats(counts: MemoryCounts): void {
	console.log(`\n${counts.total} saved memor${counts.total === 1 ? "y" : "ies"}`);
	console.log(table(["memory suggestion", "memories"], [
		["keep", counts.keep], ["delete", counts.delete], ["review", counts.review], ["not classified", counts.unclassified],
	]));
}
