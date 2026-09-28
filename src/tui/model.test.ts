import { describe, expect, test } from "bun:test";
import type { IndexedConversation } from "../index/store.ts";
import { BRAINSTORM_CYCLE, cycle, INITIAL_VIEW, type Row, visibleRows, windowStart } from "./model.ts";

const convo = (id: string, title: string, archived = false, updated = "2024-01-01T00:00:00Z"): IndexedConversation => ({
	id,
	title,
	create_time: updated,
	update_time: updated,
	is_archived: archived ? 1 : 0,
	pinned: 0,
	project_id: null,
});

const row = (
	c: IndexedConversation,
	suggestion?: "delete" | "archive" | "keep",
	topic?: "faith" | "health",
	brainstorm: "writing" | "sermon" | "product" | null = null,
): Row => ({
	c,
	judgment: null,
	verdict: suggestion ? { suggestion, unsure: false, reason: "", brainstorm } : null,
	topic: topic ?? null,
});

const rows = [
	row(convo("1", "Sprint names"), "delete", "health"),
	row(convo("2", "Devotional draft", false, "2026-09-01T00:00:00Z"), "keep", "faith", "sermon"),
	row(convo("3", "Old archived", true), "archive", "faith"),
	row(convo("4", "Unjudged chat")),
];

describe("visibleRows", () => {
	test("hides archived chats unless the archived view is on", () => {
		expect(visibleRows(rows, INITIAL_VIEW).map((r) => r.c.id)).toEqual(["1", "2", "4"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, archived: true }).map((r) => r.c.id)).toEqual(["3"]);
	});

	test("combines title query, suggestion and topic", () => {
		expect(visibleRows(rows, { ...INITIAL_VIEW, query: "DEVOTIONAL" }).map((r) => r.c.id)).toEqual(["2"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, suggestion: "delete" }).map((r) => r.c.id)).toEqual(["1"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, topic: "faith" }).map((r) => r.c.id)).toEqual(["2"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, suggestion: "unjudged" }).map((r) => r.c.id)).toEqual(["4"]);
		const torn: Row = { ...row(convo("5", "Rental car options"), "keep"), verdict: { suggestion: "keep", unsure: true, reason: "", brainstorm: null } };
		expect(visibleRows([...rows, torn], { ...INITIAL_VIEW, suggestion: "unsure" }).map((r) => r.c.id)).toEqual(["5"]);
	});
});

describe("age and brainstorm filters", () => {
	const now = Date.parse("2026-09-27T00:00:00Z");
	test("older-than keeps only chats last updated before the cutoff", () => {
		expect(visibleRows(rows, { ...INITIAL_VIEW, olderThan: "1y" }, now).map((r) => r.c.id)).toEqual(["1", "4"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, olderThan: "any" }, now).map((r) => r.c.id)).toEqual(["1", "2", "4"]);
	});

	test("brainstorm filter matches any kind or a specific one", () => {
		expect(visibleRows(rows, { ...INITIAL_VIEW, brainstorm: "any" }, now).map((r) => r.c.id)).toEqual(["2"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, brainstorm: "sermon" }, now).map((r) => r.c.id)).toEqual(["2"]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, brainstorm: "writing" }, now).map((r) => r.c.id)).toEqual([]);
		expect(visibleRows(rows, { ...INITIAL_VIEW, brainstorm: "product" }, now).map((r) => r.c.id)).toEqual([]);
		expect(cycle(BRAINSTORM_CYCLE, "writing", 1)).toBe("sermon");
	});
});

describe("windowStart", () => {
	test("scrolls only as far as needed to keep the selection visible", () => {
		expect(windowStart(0, 0, 10, 100)).toBe(0);
		expect(windowStart(9, 0, 10, 100)).toBe(0);
		expect(windowStart(10, 0, 10, 100)).toBe(1);
		expect(windowStart(3, 5, 10, 100)).toBe(3);
	});

	test("never scrolls past the end or below zero", () => {
		expect(windowStart(99, 95, 10, 100)).toBe(90);
		expect(windowStart(2, 0, 10, 3)).toBe(0);
		expect(windowStart(0, 0, 0, 5)).toBe(0);
	});
});

test("cycle wraps both ways", () => {
	expect(cycle(["a", "b", "c"], "c", 1)).toBe("a");
	expect(cycle(["a", "b", "c"], "a", -1)).toBe("c");
});
