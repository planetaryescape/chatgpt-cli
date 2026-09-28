import type { Embedder } from "./embeddings.ts";
import { SearchStore, type SearchHit } from "./store.ts";

export type SearchMode = "lexical" | "semantic" | "hybrid";

export async function searchLocal(store: SearchStore, embedder: Embedder, query: string, mode: SearchMode, limit: number, archived: boolean | null = false): Promise<SearchHit[]> {
	if (mode === "lexical") return store.lexical(query, limit, archived);
	const [vector] = await embedder.embed([query]);
	if (!vector) throw new Error("Embedding model returned no query vector.");
	if (mode === "semantic") return store.semantic(vector, embedder.version, limit, archived);

	const lexical = store.lexical(query, limit * 4, archived);
	const semantic = store.semantic(vector, embedder.version, limit * 4, archived);
	const fused = new Map<string, { hit: SearchHit; score: number }>();
	for (const [list, weight] of [[lexical, 1], [semantic, 1]] as const) {
		for (const [rank, hit] of list.entries()) {
			const previous = fused.get(hit.id);
			fused.set(hit.id, { hit: previous?.hit ?? hit, score: (previous?.score ?? 0) + weight / (60 + rank + 1) });
		}
	}
	return [...fused.values()].sort((a, b) => b.score - a.score).slice(0, limit).map(({ hit, score }) => ({ ...hit, score }));
}
