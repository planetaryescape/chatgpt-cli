import type { ChatGPTClient } from "./client.ts";

// Shapes observed in ChatGPT's Personalization screen on 2026-09-28.
export type SavedMemory = {
	id: string;
	content: string;
	updated_at: string;
	status: string;
	conversation_id: string | null;
	gizmo_id: string | null;
	created_timestamp: number;
	last_updated: string | null;
	labels: string[] | null;
};

export type MemorySummary = {
	sections: { id: string; title: string; description: string }[];
	generatedAtIso: string;
	emptyStateMessage: string | null;
	sourceChecksum: string;
};

export async function listMemories(client: ChatGPTClient): Promise<SavedMemory[]> {
	const result = await client.request<{ memories: SavedMemory[] }>("GET", "/backend-api/memories?include_memory_entries=true");
	if (!Array.isArray(result?.memories)) throw new Error("ChatGPT returned an unexpected saved-memory list.");
	return result.memories;
}

export async function getMemorySummary(client: ChatGPTClient): Promise<MemorySummary> {
	const result = await client.request<MemorySummary>("POST", "/backend-api/memories/about_you/summary?source=personalization-setting", {});
	if (!Array.isArray(result?.sections)) throw new Error("ChatGPT returned an unexpected memory summary.");
	return result;
}

export async function deleteMemory(client: ChatGPTClient, id: string): Promise<void> {
	const result = await client.request<{ success: boolean }>("DELETE", `/backend-api/memories/${encodeURIComponent(id)}`);
	if (result?.success !== true) throw new Error(`ChatGPT did not confirm deleting memory ${id}.`);
}
