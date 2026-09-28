import { homedir } from "node:os";
import { join } from "node:path";

const MODEL_ID = "Xenova/all-MiniLM-L6-v2";
const MODEL_REVISION = "751bff37182d3f1213fa05d7196b954e230abad9";
export const MODEL_VERSION = `${MODEL_ID}@${MODEL_REVISION}:q8:mean-normalized`;
export const EMBEDDING_DIM = 384;
export const EMBEDDING_BATCH_SIZE = 16;

export type Embedder = {
	readonly version: string;
	embed(texts: string[]): Promise<Float32Array[]>;
};

async function loadModel() {
	const { env, pipeline } = await import("@huggingface/transformers");
	env.cacheDir = join(process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache"), "chatgpt-cli", "models");
	return pipeline("feature-extraction", MODEL_ID, { revision: MODEL_REVISION, dtype: "q8" });
}

export class LocalEmbedder implements Embedder {
	readonly version = MODEL_VERSION;
	private model?: ReturnType<typeof loadModel>;

	async embed(texts: string[]): Promise<Float32Array[]> {
		if (!texts.length) return [];
		const model = await (this.model ??= loadModel());
		const output = await model(texts, { pooling: "mean", normalize: true });
		if (output.dims.length !== 2 || output.dims[0] !== texts.length || output.dims[1] !== EMBEDDING_DIM) {
			throw new Error(`Embedding model returned unexpected dimensions: ${output.dims.join("×")}.`);
		}
		const data = output.data;
		return texts.map((_, i) => Float32Array.from(data.slice(i * EMBEDDING_DIM, (i + 1) * EMBEDDING_DIM)));
	}
}
