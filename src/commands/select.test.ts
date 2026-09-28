import { afterEach, beforeEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { QUESTIONS_VERSION } from "../classify/questions.ts";
import { ClassificationStore } from "../index/classification-store.ts";
import { ConversationIndex } from "../index/store.ts";
import { applyJevFiltersAndLimit, selectTargets } from "./select.ts";

let dir: string;
let index: ConversationIndex;
let store: ClassificationStore;

const answers = (brainstorming: number, kind: "writing" | "sermon" = "writing") =>
	JSON.stringify({
		worth_keeping: { type: "score", score: 2, confidence: 0.9 },
		nothing_there: { type: "noul", noul: 0.02 },
		unfinished: { type: "noul", noul: 0.05 },
		personal_record: { type: "noul", noul: 0.05 },
		re_askable: { type: "noul", noul: 0.05 },
		brainstorming: { type: "noul", noul: brainstorming },
		brainstorm_for: { type: "choice", choice: brainstorming > 0.5 ? kind : "none", confidence: 0.9 },
		topic: { type: "choice", choice: "other", confidence: 0.9 },
	});

beforeEach(() => {
	dir = mkdtempSync(join(tmpdir(), "chatgpt-select-test-"));
	index = new ConversationIndex(join(dir, "index.db"));
	store = new ClassificationStore(index.db);
	// Newest first: two plain chats, then two brainstorms.
	const chats = ["plain-1", "plain-2", "idea-1", "idea-2"].map((id, i) => ({
		id,
		title: id,
		create_time: `2026-0${9 - i}-01T00:00:00Z`,
		update_time: `2026-0${9 - i}-01T00:00:00Z`,
		is_archived: false,
		pinned_time: null,
		gizmo_id: null,
	}));
	index.replaceAll(chats);
	for (const c of chats) {
		store.saveJudgment({ id: c.id, update_time: c.update_time, version: QUESTIONS_VERSION, content_kind: "full", answers: answers(c.id.startsWith("idea") ? 0.9 : 0.02), classified_at: c.update_time });
	}
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

test("--limit counts matches after the Jev filters, not before", () => {
	const rows = index.query({ includePinned: true });
	const ids = applyJevFiltersAndLimit(rows, { brainstorm: true, limit: "2" }, store).map((r) => r.id);
	expect(ids).toEqual(["idea-1", "idea-2"]);
});

test("--limit alone keeps the newest n", () => {
	const rows = index.query({ includePinned: true });
	expect(applyJevFiltersAndLimit(rows, { limit: "1" }, store).map((r) => r.id)).toEqual(["plain-1"]);
});

test("--limit rejects non-numbers", () => {
	expect(() => applyJevFiltersAndLimit([], { limit: "abc" }, store)).toThrow("--limit");
});

test("sermon brainstorms can be selected separately from other writing", () => {
	const chat = index.get("idea-1")[0]!;
	store.saveJudgment({ id: chat.id, update_time: chat.update_time, version: QUESTIONS_VERSION, content_kind: "full", answers: answers(0.9, "sermon"), classified_at: chat.update_time });
	const rows = index.query({ includePinned: true });
	expect(applyJevFiltersAndLimit(rows, { brainstorm: "sermon" }, store).map((r) => r.id)).toEqual(["idea-1"]);
	expect(applyJevFiltersAndLimit(rows, { brainstorm: "writing" }, store).map((r) => r.id)).toEqual(["idea-2"]);
});

test("explicit ids require an archived opt-in", async () => {
	index.setArchived("idea-1", true);
	await expect(selectTargets(index, ["idea-1"], {})).rejects.toThrow("--archived or --all");
	expect((await selectTargets(index, ["idea-1"], { archived: true })).map((r) => r.id)).toEqual(["idea-1"]);
	expect((await selectTargets(index, ["idea-1"], { all: true })).map((r) => r.id)).toEqual(["idea-1"]);
	await expect(selectTargets(index, ["plain-1"], { archived: true })).rejects.toThrow("active");
});
