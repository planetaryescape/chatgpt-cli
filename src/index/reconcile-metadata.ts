import type { ChatGPTClient } from "../api/client.ts";
import { BATCH_MAX, getConversationsBatch, type Conversation } from "../api/conversations.ts";
import { note, Step } from "../progress.ts";
import { RENDER_VERSION, toCachedTranscript } from "../render/transcript.ts";
import type { ConversationIndex } from "./store.ts";

type Candidate = {
	id: string;
	title: string;
	create_time: string;
	update_time: string;
	cached_update_time: string;
	markdown: string;
	turns: number;
};

export type ReconcileResult = { preserved: number; changed: number; failures: string[] };

const SEPARATOR = "\n\n---\n\n";

function sameContent(candidate: Candidate, conversation: Conversation): boolean {
	// The batch endpoint returns old or rounded update times for some legacy chats;
	// the conversation list supplies the authoritative timestamp for the index.
	if (conversation.title !== candidate.title) return false;
	const header = `# ${candidate.title}\n\nhttps://chatgpt.com/c/${candidate.id} · ${candidate.create_time.slice(0, 10)} · `;
	if (!candidate.markdown.startsWith(header)) return false;
	const fresh = toCachedTranscript(candidate.id, candidate.update_time, conversation);
	if (candidate.turns !== fresh.turns) return false;
	const oldBody = candidate.markdown.indexOf(SEPARATOR);
	const newBody = fresh.markdown.indexOf(SEPARATOR);
	return oldBody < 0 || newBody < 0
		? oldBody === newBody
		: candidate.markdown.slice(oldBody) === fresh.markdown.slice(newBody);
}

export async function reconcileMetadataChanges(
	client: ChatGPTClient,
	index: ConversationIndex,
	ids: Iterable<string>,
): Promise<ReconcileResult> {
	const db = index.db;
	const candidateQuery = db.query(`select c.id, c.title, c.create_time, c.update_time,
		t.update_time as cached_update_time, t.markdown, t.turns
		from conversations c join transcripts t on t.id = c.id
		where c.id = ? and t.update_time != c.update_time and t.render_version = ?`);
	const candidates = [...new Set(ids)].map((id) => candidateQuery.get(id, RENDER_VERSION) as Candidate | null).filter((c): c is Candidate => Boolean(c));
	const result: ReconcileResult = { preserved: 0, changed: 0, failures: [] };
	if (!candidates.length) return result;
	const step = new Step("Checking changed chat content", candidates.length);
	for (let i = 0; i < candidates.length; i += BATCH_MAX) {
		const batch = candidates.slice(i, i + BATCH_MAX);
		let fetched: Map<string, Conversation>;
		try {
			fetched = new Map((await getConversationsBatch(client, batch.map((c) => c.id))).map((c) => [c.conversation_id, c]));
		} catch (error) {
			for (const candidate of batch) result.failures.push(`${candidate.id}: ${error instanceof Error ? error.message : String(error)}`);
			step.update(Math.min(i + batch.length, candidates.length));
			continue;
		}
		for (const candidate of batch) {
			try {
				const conversation = fetched.get(candidate.id);
				if (!conversation) {
					result.failures.push(`${candidate.id}: ChatGPT did not return the chat.`);
					continue;
				}
				if (!sameContent(candidate, conversation)) {
					result.changed++;
					continue;
				}
				const updated = db.transaction(() => {
					const current = db.query("select update_time, title from conversations where id = ?").get(candidate.id) as { update_time: string; title: string } | null;
					if (current?.update_time !== candidate.update_time || current.title !== candidate.title) return false;
					const args = [candidate.update_time, candidate.id, candidate.cached_update_time];
					db.query("update transcripts set update_time = ? where id = ? and update_time = ?").run(...args);
					db.query("update summaries set update_time = ? where id = ? and update_time = ?").run(...args);
					db.query("update judgments set update_time = ? where id = ? and update_time = ?").run(...args);
					db.query("update deep_judgments set update_time = ? where id = ? and update_time = ?").run(...args);
					db.query("update luna_judgments set update_time = ? where id = ? and update_time = ?").run(...args);
					db.query("update local_titles set update_time = ? where id = ? and update_time = ?").run(...args);
					const indexed = db.query("select 1 from search_chunks where conversation_id = ? and update_time = ? and title = ? limit 1")
						.get(candidate.id, candidate.cached_update_time, candidate.title);
					if (indexed) {
						db.query("update search_indexed set update_time = ? where conversation_id = ? and update_time = ?").run(...args);
						db.query("update search_chunks set update_time = ? where conversation_id = ? and update_time = ?").run(...args);
					}
					return true;
				})();
				if (updated) result.preserved++;
				else result.changed++;
			} catch (error) {
				result.failures.push(`${candidate.id}: ${error instanceof Error ? error.message : String(error)}`);
			}
		}
		step.update(Math.min(i + batch.length, candidates.length));
		if (i + batch.length < candidates.length) await Bun.sleep(500);
	}
	step.finish(`Preserved ${result.preserved} unchanged cache(s); ${result.changed} content or metadata change(s) left stale${result.failures.length ? `, ${result.failures.length} failed` : ""}`);
	for (const failure of result.failures) console.error(`failed: ${failure}`);
	return result;
}
