import { ApiError } from "../api/client.ts";
import type { Conversation, ConversationSummary } from "../api/conversations.ts";
import type { IndexedConversation } from "./store.ts";

// The archived list can omit real chats. Confirm every previously indexed
// omission through the single-chat endpoint before a full sync removes it.
export async function reconcileFullSyncOmissions(
	listed: Map<string, ConversationSummary>,
	previous: IndexedConversation[],
	getOne: (id: string) => Promise<Conversation>,
): Promise<{ recovered: number; deleted: number }> {
	let recovered = 0;
	let deleted = 0;
	for (const old of previous) {
		if (listed.has(old.id)) continue;
		let chat: Conversation;
		try {
			chat = await getOne(old.id);
		} catch (error) {
			if (error instanceof ApiError && error.status === 404) {
				deleted++;
				continue;
			}
			throw error;
		}
		listed.set(old.id, {
			id: old.id,
			title: chat.title,
			create_time: new Date(chat.create_time * 1000).toISOString(),
			update_time: new Date(chat.update_time * 1000).toISOString(),
			is_archived: chat.is_archived ?? Boolean(old.is_archived),
			pinned_time: chat.pinned_time === undefined ? (old.pinned ? old.create_time : null) : chat.pinned_time,
			gizmo_id: chat.gizmo_id === undefined ? old.project_id : chat.gizmo_id,
		});
		recovered++;
	}
	return { recovered, deleted };
}
