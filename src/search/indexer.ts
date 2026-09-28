import type { ChatGPTClient } from "../api/client.ts";
import { BATCH_MAX, getConversationsBatch } from "../api/conversations.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import type { ConversationIndex, IndexedConversation } from "../index/store.ts";
import { note, Step } from "../progress.ts";
import { RENDER_VERSION, toCachedTranscript } from "../render/transcript.ts";
import { EMBEDDING_BATCH_SIZE, type Embedder } from "./embeddings.ts";
import { SearchStore } from "./store.ts";

const BATCH_GAP_MS = 500;

export class SearchIndexer {
	constructor(
		private readonly client: ChatGPTClient,
		private readonly index: ConversationIndex,
		private readonly transcripts: ClassificationStore,
		private readonly search: SearchStore,
		private readonly embedder: Embedder,
	) {}

	refreshCached(archived: boolean | null = false): { indexed: number; chunks: number; missing: IndexedConversation[] } {
		this.search.prune();
		let indexed = 0;
		let chunks = 0;
		const missing: IndexedConversation[] = [];
		for (const c of this.index.query({ archived: archived ?? undefined, includePinned: true })) {
			if (this.search.isCurrent(c)) continue;
			const transcript = this.transcripts.transcript(c.id, c.update_time, RENDER_VERSION);
			if (!transcript) {
				missing.push(c);
				continue;
			}
			chunks += this.search.replace(c, transcript.markdown);
			indexed++;
		}
		return { indexed, chunks, missing };
	}

	async build(archived: boolean | null = false): Promise<{ indexed: number; chunks: number; embedded: number; failures: string[] }> {
		const cached = this.refreshCached(archived);
		let indexed = cached.indexed;
		let chunks = cached.chunks;
		const failures: string[] = [];
		if (cached.missing.length) {
			const step = new Step("Downloading search transcripts", cached.missing.length);
			for (let i = 0; i < cached.missing.length; i += BATCH_MAX) {
				const batch = cached.missing.slice(i, i + BATCH_MAX);
				try {
					const found = new Map((await getConversationsBatch(this.client, batch.map((c) => c.id))).map((c) => [c.conversation_id, c]));
					for (const c of batch) {
						const conversation = found.get(c.id);
						if (!conversation) {
							failures.push(`${c.id} ${c.title}: not returned by ChatGPT; run sync.`);
							continue;
						}
						const transcript = toCachedTranscript(c.id, c.update_time, conversation);
						this.transcripts.saveTranscript(transcript);
						chunks += this.search.replace(c, transcript.markdown);
						indexed++;
					}
				} catch (err) {
					for (const c of batch) failures.push(`${c.id} ${c.title}: ${err instanceof Error ? err.message : String(err)}`);
				}
				step.update(Math.min(i + BATCH_MAX, cached.missing.length));
				if (i + BATCH_MAX < cached.missing.length) await Bun.sleep(BATCH_GAP_MS);
			}
			step.finish(`Indexed ${indexed} changed chat(s), ${chunks} chunk(s)${failures.length ? `, ${failures.length} failed` : ""}`);
		} else note(`Transcripts: ${indexed} changed chat(s), ${chunks} new chunk(s).`);

		const pending = this.search.pendingVectors(this.embedder.version, archived);
		if (!pending.length) {
			note("Embeddings: up to date.");
			return { indexed, chunks, embedded: 0, failures };
		}
		note(`Embedding ${pending.length} chunk(s) locally; the model downloads once and stays in the local cache.`);
		const step = new Step("Embedding search chunks", pending.length);
		let embedded = 0;
		for (let i = 0; i < pending.length; i += EMBEDDING_BATCH_SIZE) {
			const batch = pending.slice(i, i + EMBEDDING_BATCH_SIZE);
			const vectors = await this.embedder.embed(batch.map((c) => c.text));
			this.search.saveVectors(batch.map((c) => c.id), vectors, this.embedder.version);
			embedded += batch.length;
			step.update(embedded);
		}
		step.finish(`Embedded ${embedded} chunk(s)`);
		return { indexed, chunks, embedded, failures };
	}
}
