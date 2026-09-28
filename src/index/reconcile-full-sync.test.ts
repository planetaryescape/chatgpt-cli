import { expect, test } from "bun:test";
import { ApiError } from "../api/client.ts";
import type { Conversation, ConversationSummary } from "../api/conversations.ts";
import { reconcileFullSyncOmissions } from "./reconcile-full-sync.ts";
import type { IndexedConversation } from "./store.ts";

const row = (id: string): IndexedConversation => ({
	id, title: id, create_time: "2025-01-01T00:00:00.000Z", update_time: "2025-01-02T00:00:00.000Z",
	is_archived: 1, pinned: 1, project_id: "g-p-old",
});

const detail = (id: string): Conversation => ({
	conversation_id: id, title: `${id} current`, create_time: 1735689600, update_time: 1735776000,
	default_model_slug: null, is_archived: true, pinned_time: null, gizmo_id: "g-p-new",
	mapping: {}, current_node: "",
});

test("full sync retains real archived chats omitted by the list and drops confirmed deletes", async () => {
	const listed = new Map<string, ConversationSummary>([["listed", {
		id: "listed", title: "listed", create_time: row("listed").create_time,
		update_time: row("listed").update_time, is_archived: true, pinned_time: null, gizmo_id: null,
	}]]);
	const lookedUp: string[] = [];
	const result = await reconcileFullSyncOmissions(listed, [row("listed"), row("omitted"), row("deleted")], async (id) => {
		lookedUp.push(id);
		if (id === "deleted") throw new ApiError(404, `/conversation/${id}`, "conversation_deleted");
		return detail(id);
	});
	expect(result).toEqual({ recovered: 1, deleted: 1 });
	expect(lookedUp).toEqual(["omitted", "deleted"]);
	expect(listed.get("omitted")).toMatchObject({ title: "omitted current", is_archived: true,
		pinned_time: null, gizmo_id: "g-p-new" });
	expect(listed.has("deleted")).toBe(false);
});

test("full sync leaves the index untouched when an omitted chat cannot be checked", async () => {
	const listed = new Map<string, ConversationSummary>();
	await expect(reconcileFullSyncOmissions(listed, [row("unavailable")], async () => {
		throw new ApiError(503, "/conversation/unavailable", "unavailable");
	})).rejects.toBeInstanceOf(ApiError);
	expect(listed.size).toBe(0);
});
