import type { ChatGPTClient } from "./client.ts";

// Shapes observed from chatgpt.com traffic (2026-09-27). Only fields the CLI uses.
export type ConversationSummary = {
	id: string;
	title: string | null;
	create_time: string;
	update_time: string;
	is_archived: boolean;
	pinned_time: string | null;
	gizmo_id: string | null;
};

type ListPage = { items: ConversationSummary[] };

export type MessageNode = {
	id: string;
	parent: string | null;
	children: string[];
	message: {
		author: { role: "user" | "assistant" | "system" | "tool"; name?: string | null };
		create_time: number | null;
		// Tool the message is addressed to, e.g. "canmore.create_textdoc" for canvas.
		recipient?: string;
		content: { content_type: string; parts?: unknown[]; text?: string };
		metadata?: {
			is_visually_hidden_from_conversation?: boolean;
			attachments?: { name: string; mime_type?: string }[];
			// Inline markers (web citations, memory refs) and what the UI shows instead.
			content_references?: { matched_text: string; alt: string | null }[];
		};
	} | null;
};

export type Conversation = {
	conversation_id: string;
	title: string;
	create_time: number;
	update_time: number;
	default_model_slug: string | null;
	is_archived?: boolean;
	pinned_time?: string | null;
	gizmo_id?: string | null;
	mapping: Record<string, MessageNode>;
	current_node: string;
};

export type SearchHit = {
	title: string;
	snippet: string;
	update_time: number;
	payload: { conversation_id: string; is_archived: boolean };
};

const PAGE_SIZE = 100;

// `total` in list responses is only offset + count + 1 (a "more pages" hint),
// so the only way to find the end is to page until a short page. The list is
// ordered by update time (order=created returns 500), so pages shift when a chat
// changes mid-listing; callers must dedupe by id.
export async function* listConversations(client: ChatGPTClient, archived: boolean) {
	for (let offset = 0; ; offset += PAGE_SIZE) {
		const params = new URLSearchParams({
			offset: String(offset),
			limit: String(PAGE_SIZE),
			order: "updated",
			is_archived: String(archived),
			hide_snorlax: "false",
		});
		const page = await client.request<ListPage>("GET", `/backend-api/conversations?${params}`);
		yield* page.items;
		if (page.items.length < PAGE_SIZE) return;
	}
}

export function getConversation(client: ChatGPTClient, id: string) {
	return client.request<Conversation>("GET", `/backend-api/conversation/${id}`);
}

export async function searchConversations(client: ChatGPTClient, query: string, limit: number): Promise<SearchHit[]> {
	const res = await client.request<{ items: (SearchHit & { source_type: string })[] }>("POST", "/backend-api/global/search", {
		entrypoint: "global_search",
		limit,
		query,
		source_requests: [{ type: "conversation" }],
		cursor: null,
	});
	return res.items.filter((i) => i.source_type === "conversation");
}

export function setArchived(client: ChatGPTClient, id: string, archived: boolean) {
	return client.request("PATCH", `/backend-api/conversation/${id}`, { is_archived: archived });
}

export function renameConversation(client: ChatGPTClient, id: string, title: string) {
	return client.request("POST", `/backend-api/conversation/id/${id}/rename`, { title });
}

export function deleteConversation(client: ChatGPTClient, id: string) {
	return client.request("DELETE", `/backend-api/conversation/id/${id}`);
}

export const BATCH_MAX = 10;

type BatchItem = Omit<Conversation, "conversation_id" | "create_time" | "update_time" | "default_model_slug"> & {
	id: string;
	create_time: string;
	update_time: string;
};

// Full conversations, up to 10 per call. Unlike single fetches this isn't
// rate-limited at bulk pace (100 chats in ~43s, measured 2026-09-27). Unknown
// ids are silently left out; some deleted ids can still return tombstones.
export async function getConversationsBatch(client: ChatGPTClient, ids: string[]): Promise<Conversation[]> {
	if (ids.length > BATCH_MAX) throw new Error(`At most ${BATCH_MAX} ids per batch.`);
	const items = await client.request<BatchItem[]>("POST", "/backend-api/conversations/batch", { conversation_ids: ids });
	return items.map(({ id, create_time, update_time, ...rest }) => ({
		...rest,
		conversation_id: id,
		create_time: Date.parse(create_time) / 1000,
		update_time: Date.parse(update_time) / 1000,
		default_model_slug: null,
	}));
}
