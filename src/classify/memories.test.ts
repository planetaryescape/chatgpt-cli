import { Database } from "bun:sqlite";
import { expect, test } from "bun:test";
import type { SavedMemory } from "../api/memories.ts";
import { MemoryClassificationStore } from "../index/memory-classification-store.ts";
import { classifyMemories, finalDecision, memoryCounts, quickDecision, relatedMemories, validateDeepAnswers, type MemoryAnswers } from "./memories.ts";

const memory = (id: string, content: string): SavedMemory => ({ id, content, updated_at: "2026-09-28T14:00:00Z",
	status: "warm", conversation_id: null, gizmo_id: null, created_timestamp: 1, last_updated: null, labels: null });

function answers(score: number, expired = 0, redundant = 0): MemoryAnswers {
	return { lasting_value: { type: "score", score, confidence: 0.9 },
		expired: { type: "noul", noul: expired }, superseded: { type: "noul", noul: 0 },
		redundant: { type: "noul", noul: redundant } } as MemoryAnswers;
}

test("quick stage only keeps strong enduring context; uncertain deletion gets deep review", () => {
	expect(quickDecision(answers(3)).suggestion).toBe("keep");
	expect(quickDecision(answers(0, 0.99)).suggestion).toBe("review");
	expect(quickDecision(answers(3, 0, 0.7)).suggestion).toBe("review");
});

test("conflicting strong lasting-value evidence prevents a deep delete recommendation", () => {
	expect(finalDecision(answers(2.5), { suggestion: "delete", reason: "Narrow detail" }).suggestion).toBe("review");
	expect(finalDecision(answers(2.5, 0, 0.8), { suggestion: "delete", reason: "Full duplicate" }).suggestion).toBe("delete");
});

test("related memories expose strong overlap without treating a shared topic as a duplicate", () => {
	const a = memory("a", "A user writes a blog about things they learn");
	const b = memory("b", "A user likes writing blog posts about things they learn");
	const c = memory("c", "A user is growing tomatoes in a garden");
	expect(relatedMemories(a, [a, b, c]).map((m) => m.id)).toEqual(["b"]);
	const long = memory("long", `${"garden planting "} ${"unrelated fantasy details ".repeat(80)}`);
	expect(relatedMemories(c, [c, long])).toEqual([]);
});

test("deep results must contain the exact batch in order", () => {
	expect(() => validateDeepAnswers(["a", "b"], { decisions: [{ id: "a", suggestion: "delete", reason: "Expired" }] }))
		.toThrow("incomplete");
});

test("memory classification caches decisions and routes uncertain entries to Luna", async () => {
	const db = new Database(":memory:");
	const store = new MemoryClassificationStore(db);
	const rows = [memory("lasting", "The user prefers dark mode"), memory("temporary", "The user is preparing tomorrow's event")];
	let quickCalls = 0;
	let deepCalls = 0;
	const quick = async (state: unknown) => {
		quickCalls++;
		return (state as { memory: { id: string } }).memory.id === "lasting" ? answers(3) : answers(0, 0.95);
	};
	const deep = async (input: unknown) => {
		deepCalls++;
		const ids = (input as { memories: { id: string }[] }).memories.map((m) => m.id);
		return { decisions: ids.map((id) => ({ id, suggestion: "delete" as const, reason: "The one-off event passed." })) };
	};
	const first = await classifyMemories(rows, store, { quick, deep, asOf: "2026-09-28" });
	expect(first.failures).toEqual([]);
	expect(first.rows.map((r) => [r.suggestion, r.stage])).toEqual([["keep", "quick"], ["delete", "deep"]]);
	const again = await classifyMemories(rows, store, { quick, deep, asOf: "2026-09-28" });
	expect(again.rows).toEqual(first.rows);
	expect(quickCalls).toBe(2);
	expect(deepCalls).toBe(1);
	expect(memoryCounts(rows, store, "2026-09-28")).toEqual({ total: 2, keep: 1, delete: 1, review: 0, unclassified: 0 });
	expect(memoryCounts([rows[0] as SavedMemory, memory("new", "A new fact")], store, "2026-09-28").unclassified).toBe(1);
	db.close();
});

test("an incomplete Luna batch retries each memory and saves complete decisions", async () => {
	const db = new Database(":memory:");
	const store = new MemoryClassificationStore(db);
	const rows = [memory("a", "A past task"), memory("b", "Another past task")];
	let deepCalls = 0;
	const result = await classifyMemories(rows, store, { asOf: "2026-09-28", quick: async () => answers(0),
		deep: async (input) => {
			deepCalls++;
			const ids = (input as { memories: { id: string }[] }).memories.map((m) => m.id);
			return { decisions: ids.length > 1 ? [] : [{ id: ids[0] as string, suggestion: "delete" as const, reason: "Temporary task" }] };
		} });
	expect(deepCalls).toBe(3);
	expect(result.failures).toEqual([]);
	expect(result.rows.map((r) => r.suggestion)).toEqual(["delete", "delete"]);
	db.close();
});
