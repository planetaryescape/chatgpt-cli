// Embeds texts with the TS CLI's own LocalEmbedder, so the Rust embedder can
// be checked against it (docs/explanation/embeddings.md).
//
//   bun scripts/dump-embeddings.ts [--batch <n>] < texts.json > vectors.json
//
// Input: a JSON array of strings. Output: a JSON array of 384-number arrays,
// in the same order. Texts are embedded n at a time (default
// EMBEDDING_BATCH_SIZE, as `search-index` does); the quantised model scales
// each batch as a whole, so a vector depends on the texts beside it.
import { parseArgs } from "node:util";
import { EMBEDDING_BATCH_SIZE, LocalEmbedder } from "../src/search/embeddings.ts";

function parseTexts(raw: string): string[] {
	const parsed: unknown = JSON.parse(raw);
	if (!Array.isArray(parsed) || !parsed.every((item): item is string => typeof item === "string")) {
		throw new Error("Expected a JSON array of strings on stdin.");
	}
	return parsed;
}

function parseBatch(raw: string | undefined): number {
	if (raw === undefined) return EMBEDDING_BATCH_SIZE;
	const batch = Number(raw);
	if (!Number.isSafeInteger(batch) || batch < 1) throw new Error("--batch must be a positive integer.");
	return batch;
}

const { values } = parseArgs({ options: { batch: { type: "string" } } });
const batchSize = parseBatch(values.batch);
const texts = parseTexts(await Bun.stdin.text());
const embedder = new LocalEmbedder();
const vectors: number[][] = [];
for (let i = 0; i < texts.length; i += batchSize) {
	const batch = await embedder.embed(texts.slice(i, i + batchSize));
	vectors.push(...batch.map((vector) => Array.from(vector)));
}
console.log(JSON.stringify(vectors));
