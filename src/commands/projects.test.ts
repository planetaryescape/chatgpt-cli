import { expect, test } from "bun:test";
import { ApiError, type ChatGPTClient } from "../api/client.ts";
import { addConversationToProject, createProject, listProjects, removeConversationFromProject, type Project } from "../api/projects.ts";
import { ConversationIndex } from "../index/store.ts";
import { applyProjectAdd, applyProjectRemove, resolveProject } from "./projects.ts";

const projects: Project[] = [
	{ id: "g-p-one111", name: "Garden Planner", canWrite: true },
	{ id: "g-p-two222", name: "Reading Club", canWrite: true },
];

test("project creation uses the observed ChatGPT request and returns its id", async () => {
	const calls: { method: string; path: string; body?: unknown }[] = [];
	const client = { async request(method: string, path: string, body?: unknown) {
		calls.push({ method, path, body });
		return method === "GET"
			? { items: [], cursor: null }
			: { resource: { gizmo: { id: "g-p-created", display: { name: "Research Notes" }, current_user_permission: { can_write: true } } } };
	} } as ChatGPTClient;
	expect(await createProject(client, "  Research Notes  ")).toEqual({ id: "g-p-created", name: "Research Notes", canWrite: true });
	expect(calls[1]).toEqual({ method: "POST", path: "/backend-api/projects", body: {
		emoji: null, instructions: "", memory_scope: "unset", name: "Research Notes", theme: null,
	} });
});

test("project creation rejects empty and duplicate names before posting", async () => {
	const methods: string[] = [];
	const existing = projects[0] as Project;
	const client = { async request(method: string, _path: string, _body?: unknown) {
		methods.push(method);
		return { items: [{ gizmo: { gizmo: {
			id: existing.id, display: { name: existing.name },
			current_user_permission: { can_write: true }, is_archived: false,
		} } }], cursor: null };
	} } as ChatGPTClient;
	await expect(createProject(client, "  ")).rejects.toThrow("cannot be empty");
	await expect(createProject(client, "garden planner")).rejects.toThrow("already exists");
	expect(methods).toEqual(["GET"]);
});

test("project listing reads every cursor and keeps ids, names and permissions", async () => {
	const paths: string[] = [];
	const client = { async request(_method: string, path: string) {
		paths.push(path);
		const project = paths.length === 1 ? projects[0] as Project : projects[1] as Project;
		return { items: [{ gizmo: { gizmo: {
			id: project.id, display: { name: project.name },
			current_user_permission: { can_write: project.canWrite }, is_archived: false,
		} } }], cursor: paths.length === 1 ? "next page" : null };
	} } as ChatGPTClient;
	expect(await listProjects(client)).toEqual(projects);
	expect(paths[0]).toContain("conversations_per_gizmo=0");
	expect(paths[1]).toContain("cursor=next+page");
});

test("project resolution accepts exact names and unique id prefixes, but rejects ambiguity", () => {
	expect(resolveProject(projects, "garden planner").id).toBe("g-p-one111");
	expect(resolveProject(projects, "g-p-two").name).toBe("Reading Club");
	expect(() => resolveProject(projects, "g-p-")).toThrow("matches 2 projects");
	expect(() => resolveProject(projects, "missing")).toThrow("project list");
});

test("a read-only project is rejected before any move", async () => {
	const index = new ConversationIndex(":memory:");
	try {
		index.db.query("insert into conversations values (?, ?, ?, ?, 0, 0, null)")
			.run("chat", "Test chat", "2026-09-28T00:00:00Z", "2026-09-28T00:00:00Z");
		let calls = 0;
		const client = { async request(_method: string, _path: string) { calls++; return { success: true }; } } as ChatGPTClient;
		await expect(applyProjectAdd(client, index, { ...projects[0] as Project, canWrite: false }, index.query({ includePinned: true }), { yes: true }))
			.rejects.toThrow("do not have write access");
		expect(calls).toBe(0);
	} finally {
		index.db.close();
	}
});

test("a legacy 500 is confirmed by reading the chat's project", async () => {
	const calls: string[] = [];
	const path = "/backend-api/conversation/legacy-chat";
	const client = { async request(method: string, requestedPath: string) {
		calls.push(`${method} ${requestedPath}`);
		if (method === "PATCH") throw new ApiError(500, requestedPath, '{"detail":"Something went wrong."}');
		return { gizmo_id: "g-p-one111" };
	} } as ChatGPTClient;
	await addConversationToProject(client, "legacy-chat", "g-p-one111");
	expect(calls).toEqual([`PATCH ${path}`, `GET ${path}`]);
});

test("a legacy 500 remains a failure if the chat is outside the project", async () => {
	const client = { async request(method: string, path: string) {
		if (method === "PATCH") throw new ApiError(500, path, '{"detail":"Something went wrong."}');
		return { gizmo_id: null };
	} } as ChatGPTClient;
	await expect(addConversationToProject(client, "legacy-chat", "g-p-one111")).rejects.toThrow("500");
});

test("a legacy 500 confirms removal when the chat has no project", async () => {
	const path = "/backend-api/conversation/legacy-chat";
	const client = { async request(method: string, requestedPath: string, body?: { gizmo_id: string }) {
		if (method === "PATCH") {
			expect(requestedPath).toBe(path);
			expect(body).toEqual({ gizmo_id: "" });
			throw new ApiError(500, requestedPath, '{"detail":"Something went wrong."}');
		}
		return { gizmo_id: null };
	} } as ChatGPTClient;
	await removeConversationFromProject(client, "legacy-chat");
});

test("a project move updates the index only after ChatGPT confirms it and skips chats already there", async () => {
	const index = new ConversationIndex(":memory:");
	try {
		for (const [id, projectId] of [["new", null], ["already", "g-p-one111"]] as const) {
			index.db.query("insert into conversations values (?, ?, ?, ?, 0, 0, ?)")
				.run(id, id, "2026-09-28T00:00:00Z", "2026-09-28T00:00:00Z", projectId);
		}
		const calls: string[] = [];
		const client = { async request(_method: string, path: string, body: { gizmo_id: string }) {
			calls.push(`${path} ${body.gizmo_id}`);
			return { success: true };
		} } as ChatGPTClient;
		await applyProjectAdd(client, index, projects[0] as Project, index.query({ includePinned: true }), { yes: true });
		expect(calls).toEqual(["/backend-api/conversation/new g-p-one111"]);
		expect(index.get("new")[0]?.project_id).toBe("g-p-one111");
		await applyProjectAdd(client, index, projects[0] as Project, index.query({ includePinned: true }), { yes: true });
		expect(calls).toHaveLength(1);
	} finally {
		index.db.close();
	}
});

test("a rejected move leaves the local project unchanged and reports failure", async () => {
	const index = new ConversationIndex(":memory:");
	const previousExitCode = process.exitCode;
	try {
		index.db.query("insert into conversations values (?, ?, ?, ?, 0, 0, null)")
			.run("chat", "Test chat", "2026-09-28T00:00:00Z", "2026-09-28T00:00:00Z");
		const client = { async request(_method: string, _path: string) { return { success: false }; } } as ChatGPTClient;
		await applyProjectAdd(client, index, projects[0] as Project, index.query({ includePinned: true }), { yes: true });
		expect(index.get("chat")[0]?.project_id).toBeNull();
		expect(process.exitCode).toBe(1);
	} finally {
		process.exitCode = previousExitCode ?? 0;
		index.db.close();
	}
});

test("project removal only changes chats in the named project after confirmation", async () => {
	const index = new ConversationIndex(":memory:");
	try {
		for (const [id, projectId] of [["target", "g-p-one111"], ["other", "g-p-two222"], ["none", null]] as const) {
			index.db.query("insert into conversations values (?, ?, ?, ?, 0, 0, ?)")
				.run(id, id, "2026-09-28T00:00:00Z", "2026-09-28T00:00:00Z", projectId);
		}
		const calls: string[] = [];
		const client = { async request(method: string, path: string, body: { gizmo_id: string }) {
			calls.push(`${method} ${path} ${body.gizmo_id}`);
			return { success: true };
		} } as ChatGPTClient;
		const targets = index.query({ includePinned: true });
		await applyProjectRemove(client, index, projects[0] as Project, targets, { dryRun: true });
		expect(calls).toHaveLength(0);
		await applyProjectRemove(client, index, projects[0] as Project, targets, { yes: true });
		expect(calls).toEqual(["PATCH /backend-api/conversation/target "]);
		expect(index.get("target")[0]?.project_id).toBeNull();
		expect(index.get("other")[0]?.project_id).toBe("g-p-two222");
	} finally {
		index.db.close();
	}
});
