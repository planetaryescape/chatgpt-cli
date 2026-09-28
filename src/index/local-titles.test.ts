import { afterAll, afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ConversationIndex, displayTitle } from "./store.ts";
import { ClassificationStore } from "./classification-store.ts";
import { LUNA_JUDGMENT_VERSION } from "../classify/luna-version.ts";

const dir = mkdtempSync(join(tmpdir(), "chatgpt-local-titles-test-"));
const index = new ConversationIndex(join(dir, "index.db"));
const chat = (update_time: string) => ({ id: "chat-1", title: "Original title", create_time: "2024-01-01T00:00:00Z",
	update_time, is_archived: false, pinned_time: null, gizmo_id: null });
afterEach(() => index.db.run("delete from local_titles"));

test("generated local titles are searchable and become stale when content changes", () => {
	index.replaceAll([chat("2024-01-01T00:00:00Z")]);
	index.setLocalTitle(index.get("chat-1")[0]!, "Accurate local name", "topic", "luna");
	expect(displayTitle(index.get("chat-1")[0]!)).toBe("Accurate local name");
	expect(index.query({ includePinned: true, titleMatch: /accurate/i })).toHaveLength(1);
	expect(index.query({ includePinned: true, titleMatch: /original/i })).toHaveLength(1);
	expect(index.query({ includePinned: true, updatedBefore: new Date("2024-01-02T00:00:00Z") })).toHaveLength(1);
	index.replaceAll([chat("2024-01-02T00:00:00Z")]);
	expect(displayTitle(index.get("chat-1")[0]!)).toBe("Original title");
	expect(index.localTitle(index.get("chat-1")[0]!)).toBeNull();
});

test("manual title survives a sync and takes priority over generated titles", () => {
	index.replaceAll([chat("2024-01-01T00:00:00Z")]);
	index.setLocalTitle(index.get("chat-1")[0]!, "My chosen title");
	index.replaceAll([chat("2024-01-02T00:00:00Z")]);
	expect(displayTitle(index.get("chat-1")[0]!)).toBe("My chosen title");
	expect(index.localTitle(index.get("chat-1")[0]!)?.source).toBe("manual");
});

test("topic is queryable directly from the judgment table", () => {
	const store = new ClassificationStore(index.db);
	store.saveJudgment({ id: "chat-1", update_time: "2024-01-02T00:00:00Z", version: "test", content_kind: "full",
		answers: JSON.stringify({ topic: { choice: "faith" } }), classified_at: "2024-01-02T00:00:00Z" });
	const row = index.db.query("select topic from judgments where id = 'chat-1'").get() as { topic: string };
	expect(row.topic).toBe("faith");
});

test("Luna product review joins a sure judgment without a Jev follow-up", () => {
	const store = new ClassificationStore(index.db);
	store.saveJudgment({ id: "product-review", update_time: "2024-01-02", version: "test", content_kind: "full",
		answers: JSON.stringify({ topic: { choice: "side_projects" } }), classified_at: "2024-01-02" });
	store.saveLunaJudgment({ id: "product-review", update_time: "2024-01-02", questions_version: "test", deep_version: "",
		version: LUNA_JUDGMENT_VERSION, suggestion: "keep", brainstorm: "product", reason: "the user's own idea" });
	expect(store.judgment("product-review", "2024-01-02", "test")).toMatchObject({
		luna_version: LUNA_JUDGMENT_VERSION, luna_brainstorm: "product",
	});
});

afterAll(() => {
	index.db.close();
	rmSync(dir, { recursive: true, force: true });
});
