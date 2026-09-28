import { expect, test } from "bun:test";
import { ClassificationStore } from "../index/classification-store.ts";
import { ConversationIndex } from "../index/store.ts";
import { RENDER_VERSION } from "../render/transcript.ts";
import { MAX_CHUNK_CHARS, transcriptChunks } from "./chunks.ts";
import type { Embedder } from "./embeddings.ts";
import { SearchIndexer } from "./indexer.ts";
import { searchLocal } from "./query.ts";
import { SearchStore } from "./store.ts";

const date = "2026-09-28T10:00:00Z";
const later = "2026-09-28T11:00:00Z";

const fakeEmbedder: Embedder = {
	version: "fixture-v1",
	async embed(texts) {
		return texts.map((text) => {
			const vector = new Float32Array(384);
			if (/vehicle|engine|mechanic|car/i.test(text)) vector[0] = 1;
			if (/fruit|banana|bread/i.test(text)) vector[1] = 1;
			return vector;
		});
	},
};

function setup() {
	const index = new ConversationIndex(":memory:");
	const transcripts = new ClassificationStore(index.db);
	const search = new SearchStore(index.db);
	const add = (id: string, title: string, body: string, updated = date) => {
		index.db.query("insert or replace into conversations values (?, ?, ?, ?, 0, 0, null)").run(id, title, date, updated);
		transcripts.saveTranscript({ id, update_time: updated, render_version: RENDER_VERSION, markdown: `# ${title}\n\n---\n\n## Me\n\n${body}`, turns: 1, approx_tokens: 20 });
	};
	const indexer = new SearchIndexer(null as never, index, transcripts, search, fakeEmbedder);
	return { index, transcripts, search, add, indexer };
}

test("chunks retain late content and fit the local model input", () => {
	const chunks = transcriptChunks(`# Test\n\n---\n\n## Me\n\n${"word ".repeat(400)}late-marker`);
	expect(chunks.length).toBeGreaterThan(1);
	expect(chunks.every((c) => c.length <= MAX_CHUNK_CHARS)).toBe(true);
	expect(chunks.at(-1)).toContain("late-marker");
});

test("local FTS and embeddings find distinct chats, deduplicate chunks, and resume without work", async () => {
	const { index, search, add, indexer } = setup();
	try {
		add("car", "Car repair", `${"The mechanic checked the engine. ".repeat(40)}The radiator leaked.`);
		add("food", "Cooking notes", "A banana is a fruit. Make bread with it.");
		const first = await indexer.build();
		expect(first.indexed).toBe(2);
		expect(first.embedded).toBeGreaterThan(2);
		expect(search.lexical("radiator", 10).map((h) => h.id)).toEqual(["car"]);
		expect(search.lexical("engine", 10).map((h) => h.id)).toEqual(["car"]);
		expect(search.lexical("engine", 10)[0]?.score).toBeGreaterThan(0);
		expect(search.lexical("vehicle", 10)).toEqual([]);
		expect((await searchLocal(search, fakeEmbedder, "vehicle", "semantic", 1))[0]?.id).toBe("car");
		const hybrid = (await searchLocal(search, fakeEmbedder, "banana", "hybrid", 1))[0];
		expect(hybrid?.id).toBe("food");
		expect(hybrid?.score).toBeCloseTo(2 / 61);
		expect(await indexer.build()).toMatchObject({ indexed: 0, embedded: 0, failures: [] });
	} finally {
		index.db.close();
	}
});

test("changed and removed conversations cannot leak stale search results", async () => {
	const { index, search, add, indexer } = setup();
	try {
		add("chat", "Old title", "A blue widget is here.");
		await indexer.build();
		expect(search.lexical("blue", 5)).toHaveLength(1);
		index.db.query("update conversations set update_time = ? where id = ?").run(later, "chat");
		expect(search.lexical("blue", 5)).toHaveLength(0);
		add("chat", "New title", "A green widget is here.", later);
		await indexer.build();
		expect(search.lexical("blue", 5)).toHaveLength(0);
		expect(search.lexical("green", 5)).toHaveLength(1);
		index.remove("chat");
		expect(search.lexical("green", 5)).toHaveLength(0);
		expect(search.prune()).toBe(1);
		expect(search.coverage(fakeEmbedder.version)).toMatchObject({ chats: 0, indexed: 0, chunks: 0, embedded: 0 });
	} finally {
		index.db.close();
	}
});

test("a title change refreshes indexed title even when update time is unchanged", () => {
	const { index, search, add, indexer } = setup();
	try {
		add("chat", "Old subject", "Some notes.");
		indexer.refreshCached();
		index.rename("chat", "New subject");
		expect(indexer.refreshCached().indexed).toBe(1);
		expect(search.lexical("old", 5)).toHaveLength(0);
		expect(search.lexical("new", 5)[0]?.title).toBe("New subject");
	} finally {
		index.db.close();
	}
});

test("search input is treated as terms and rejects nonword queries", () => {
	const { index, search, add, indexer } = setup();
	try {
		add("chat", "C++ help", "A parser handles C++ syntax.");
		indexer.refreshCached();
		expect(search.lexical("C++", 5).map((h) => h.id)).toEqual(["chat"]);
		expect(() => search.lexical("???", 5)).toThrow("letter or number");
	} finally {
		index.db.close();
	}
});

test("lexical, semantic and hybrid search scope archived chats before ranking", async () => {
	const { index, search, add, indexer } = setup();
	try {
		add("active", "Engine active", "A mechanic checked this engine.");
		add("archived", "Engine archived", "A mechanic checked this engine.");
		await indexer.build();
		index.setArchived("archived", true);
		for (const mode of ["lexical", "semantic", "hybrid"] as const) {
			const query = mode === "semantic" ? "vehicle" : "engine";
			expect((await searchLocal(search, fakeEmbedder, query, mode, 1)).map((h) => h.id)).toEqual(["active"]);
			expect((await searchLocal(search, fakeEmbedder, query, mode, 1, true)).map((h) => h.id)).toEqual(["archived"]);
			expect((await searchLocal(search, fakeEmbedder, query, mode, 2, null)).map((h) => h.id).sort()).toEqual(["active", "archived"]);
		}
	} finally {
		index.db.close();
	}
});

test("search indexing skips archived chats until explicitly requested", async () => {
	const { index, search, add, indexer } = setup();
	try {
		add("active", "Active note", "A mechanic checked the engine.");
		add("archived", "Archived note", "A banana is a fruit.");
		index.setArchived("archived", true);
		expect(await indexer.build()).toMatchObject({ indexed: 1, embedded: 1 });
		expect(search.coverage(fakeEmbedder.version)).toMatchObject({ chats: 1, indexed: 1, embedded: 1 });
		expect(search.lexical("banana", 5, true)).toEqual([]);
		expect(await indexer.build(true)).toMatchObject({ indexed: 1, embedded: 1 });
		expect(search.lexical("banana", 5, true).map((h) => h.id)).toEqual(["archived"]);
	} finally {
		index.db.close();
	}
});
