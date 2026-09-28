import { ApiError, type ChatGPTClient } from "./client.ts";

export type Project = { id: string; name: string; canWrite: boolean };

type SidebarProject = {
	id?: string;
	display?: { name?: string };
	current_user_permission?: { can_write?: boolean };
	is_archived?: boolean;
};

type ProjectPage = {
	items: { gizmo?: { gizmo?: SidebarProject } }[];
	cursor: string | null;
};

export async function listProjects(client: ChatGPTClient): Promise<Project[]> {
	const projects: Project[] = [];
	const seenCursors = new Set<string>();
	const seenProjects = new Set<string>();
	let cursor: string | null = null;
	for (;;) {
		const params = new URLSearchParams({ conversations_per_gizmo: "0", limit: "20", owned_only: "false" });
		if (cursor) params.set("cursor", cursor);
		const page = await client.request<ProjectPage>("GET", `/backend-api/gizmos/snorlax/sidebar?${params}`);
		if (!Array.isArray(page?.items)) throw new Error("ChatGPT returned an invalid project list.");
		for (const item of page.items) {
			const gizmo = item.gizmo?.gizmo;
			if (!gizmo?.id?.startsWith("g-p-") || typeof gizmo.display?.name !== "string") {
				throw new Error("ChatGPT returned a project with no id or name.");
			}
			if (!gizmo.is_archived && !seenProjects.has(gizmo.id)) projects.push({
				id: gizmo.id,
				name: gizmo.display.name,
				canWrite: gizmo.current_user_permission?.can_write === true,
			});
			seenProjects.add(gizmo.id);
		}
		if (!page.cursor) return projects;
		if (seenCursors.has(page.cursor)) throw new Error("ChatGPT repeated a project list cursor.");
		seenCursors.add(page.cursor);
		cursor = page.cursor;
	}
}

export async function addConversationToProject(client: ChatGPTClient, conversationId: string, projectId: string): Promise<void> {
	await setConversationProject(client, conversationId, projectId);
}

export async function removeConversationFromProject(client: ChatGPTClient, conversationId: string): Promise<void> {
	await setConversationProject(client, conversationId, "");
}

async function setConversationProject(client: ChatGPTClient, conversationId: string, projectId: string): Promise<void> {
	const path = `/backend-api/conversation/${conversationId}`;
	let result: { success?: boolean };
	try {
		result = await client.request<{ success?: boolean }>("PATCH", path, { gizmo_id: projectId });
	} catch (error) {
		// Older chats can return 500 after the project move has already applied.
		if (error instanceof ApiError && error.status === 500) {
			const chat = await client.request<{ gizmo_id?: string | null }>("GET", path);
			if ((chat?.gizmo_id ?? "") === projectId) return;
		}
		throw error;
	}
	if (result?.success !== true) throw new Error(`ChatGPT did not confirm changing the project of ${conversationId}.`);
}
