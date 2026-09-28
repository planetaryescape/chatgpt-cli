import type { Classifier } from "../classify/pipeline.ts";
import { approves, verdictOf } from "../classify/policy.ts";
import { displayTitle, type IndexedConversation } from "../index/store.ts";
import { note } from "../progress.ts";

// Asks Jev about each target and keeps only those it backs for `action`.
// Anything it can't judge (download or model failure) is held back, never approved.
export async function checkWithJev(
	classifier: Classifier,
	action: "archive" | "delete",
	targets: IndexedConversation[],
	opts: { yes?: boolean },
): Promise<IndexedConversation[]> {
	const { judgments, failures } = await classifier.classify(targets, opts);
	const approved: IndexedConversation[] = [];
	const held: string[] = [];
	for (const c of targets) {
		const j = judgments.get(c.id);
		if (!j) {
			held.push(`  not judged  ${displayTitle(c)}`);
			continue;
		}
		const v = verdictOf(j);
		if (approves(action, v)) approved.push(c);
		else held.push(`  ${`${v.suggestion}${v.unsure ? "?" : ""}`.padEnd(10)}  ${displayTitle(c)}  (${v.reason})`);
	}
	for (const f of failures) console.error(`failed: ${f}`);
	note(`Classification backs ${action} for ${approved.length} of ${targets.length}.`);
	if (held.length) note(`Held back (suggestion, then the title):\n${held.join("\n")}`);
	return approved;
}
