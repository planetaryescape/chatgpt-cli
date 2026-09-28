import { expect, test } from "bun:test";
import type { ChatGPTClient } from "../api/client.ts";
import { deleteMemory, getMemorySummary, listMemories, type SavedMemory } from "../api/memories.ts";
import { deleteSelectedMemories, formatMemories, resolveMemories } from "./memories.ts";

const memory = (id: string, content: string): SavedMemory => ({
	id, content, updated_at: "2026-09-28T14:00:00Z", status: "warm", conversation_id: null,
	gizmo_id: null, created_timestamp: 1, last_updated: null, labels: null,
});

test("saved memories and the separate summary use the observed endpoints", async () => {
	const calls: [string, string, unknown][] = [];
	const client = { request: async (method: string, path: string, body?: unknown) => {
		calls.push([method, path, body]);
		return path.includes("about_you") ? { sections: [] } : { memories: [memory("one", "Remember this")] };
	} } as unknown as ChatGPTClient;
	expect(await listMemories(client)).toHaveLength(1);
	expect((await getMemorySummary(client)).sections).toEqual([]);
	expect(calls).toEqual([
		["GET", "/backend-api/memories?include_memory_entries=true", undefined],
		["POST", "/backend-api/memories/about_you/summary?source=personalization-setting", {}],
	]);
});

test("memory output is pipeable and preserves full content in JSON and CSV", () => {
	const row = memory("abc", 'Line one, "quoted"\nLine two');
	expect(JSON.parse(formatMemories([row], "json"))).toEqual([row]);
	expect(formatMemories([row], "csv")).toContain('"Line one, ""quoted""\nLine two"');
	expect(formatMemories([row], "ids")).toBe("abc");
	expect(formatMemories([], "ids")).toBe("");
});

test("delete resolves unique ids from a fresh list and dry run makes no DELETE request", async () => {
	const rows = [memory("aaaa1111", "First"), memory("aaab2222", "Second")];
	const calls: string[] = [];
	const client = { request: async (method: string, path: string) => {
		calls.push(`${method} ${path}`);
		if (method === "GET") return { memories: rows };
		return { success: true };
	} } as unknown as ChatGPTClient;
	await expect(deleteSelectedMemories(client, ["aaaa"], { dryRun: true })).resolves.toBeUndefined();
	expect(calls).toEqual(["GET /backend-api/memories?include_memory_entries=true"]);
	await expect(deleteSelectedMemories(client, ["aaaa", "aaaa"], { yes: true })).resolves.toBeUndefined();
	expect(calls.at(-1)).toBe("DELETE /backend-api/memories/aaaa1111");
	expect(() => resolveMemories(rows, ["aaa"])).toThrow("matches 2");
	expect(() => resolveMemories(rows, ["missing"])).toThrow("No saved memory");
});

test("a failed delete response is not reported as success", async () => {
	const client = { request: async () => ({ success: false }) } as unknown as ChatGPTClient;
	await expect(deleteMemory(client, "memory-id")).rejects.toThrow("did not confirm");
});
