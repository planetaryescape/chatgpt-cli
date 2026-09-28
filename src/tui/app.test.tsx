import { afterEach, beforeEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { testRender } from "@opentui/react/test-utils";
import type { ChatGPTClient } from "../api/client.ts";
import { QUESTIONS_VERSION } from "../classify/questions.ts";
import { ClassificationStore } from "../index/classification-store.ts";
import { ConversationIndex } from "../index/store.ts";
import { RENDER_VERSION } from "../render/transcript.ts";
import { App } from "./app.tsx";

const T = "2024-05-01T10:00:00Z";
let dir: string;
let index: ConversationIndex;
let store: ClassificationStore;

// Fixture answers shaped like Jev's; policy.ts turns them into suggestions.
const answers = (nothing: number, worth: number) =>
	JSON.stringify({
		worth_keeping: { type: "score", score: worth, confidence: 0.9 },
		nothing_there: { type: "noul", noul: nothing },
		unfinished: { type: "noul", noul: 0.05 },
		personal_record: { type: "noul", noul: 0.05 },
		re_askable: { type: "noul", noul: 0.05 },
		brainstorming: { type: "noul", noul: 0.02 },
		brainstorm_for: { type: "choice", choice: "none", confidence: 0.9 },
		topic: { type: "choice", choice: "coding_general", confidence: 0.9 },
	});

beforeEach(() => {
	dir = mkdtempSync(join(tmpdir(), "chatgpt-tui-test-"));
	index = new ConversationIndex(join(dir, "index.db"));
	store = new ClassificationStore(index.db);
	index.replaceAll([
		{ id: "a1", title: "Empty test chat", create_time: T, update_time: T, is_archived: false, pinned_time: null, gizmo_id: null },
		{ id: "b2", title: "Postgres backup plan", create_time: T, update_time: T, is_archived: false, pinned_time: null, gizmo_id: null },
		{ id: "c3", title: "Unjudged third chat", create_time: T, update_time: T, is_archived: false, pinned_time: null, gizmo_id: null },
	]);
	store.saveJudgment({ id: "a1", update_time: T, version: QUESTIONS_VERSION, content_kind: "full", answers: answers(0.95, 0.1), classified_at: T });
	store.saveJudgment({ id: "b2", update_time: T, version: QUESTIONS_VERSION, content_kind: "full", answers: answers(0.02, 2.6), classified_at: T });
	for (const id of ["a1", "b2", "c3"]) {
		store.saveTranscript({ id, update_time: T, render_version: RENDER_VERSION, markdown: `# chat ${id}\n\nhello from ${id}`, turns: 2, approx_tokens: 10 });
	}
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

// Transcripts are cached, so the client must never be called.
const client = {} as ChatGPTClient;

// Key events update React state asynchronously; let it commit before rendering.
async function settle(setup: Awaited<ReturnType<typeof testRender>>) {
	await Bun.sleep(20);
	await setup.renderOnce();
}

test("lists chats with Jev's suggestions and previews the selection", async () => {
	const setup = await testRender(<App client={client} index={index} store={store} />, { width: 140, height: 20 });
	try {
		await settle(setup);
		const frame = setup.captureCharFrame();
		expect(frame).toContain("3 of 3");
		expect(frame).toContain("updated 2024-05-01");
		expect(frame).toContain("Jev: DELETE · coding_general");
		expect(frame).toContain("delete");
		expect(frame).toContain("keep");
		expect(frame).toContain("hello from");
	} finally {
		setup.renderer.destroy();
	}
});

test("filters by suggestion and marks the selection", async () => {
	const setup = await testRender(<App client={client} index={index} store={store} />, { width: 140, height: 20 });
	try {
		setup.mockInput.pressKey("2");
		await settle(setup);
		let frame = setup.captureCharFrame();
		expect(frame).toContain("1 of 3");
		expect(frame).toContain("Empty test chat");
		expect(frame).not.toContain("Postgres backup plan");

		setup.mockInput.pressEnter();
		await settle(setup);
		frame = setup.captureCharFrame();
		expect(frame).toContain("marked: 1 delete, 0 archive");
	} finally {
		setup.renderer.destroy();
	}
});

test("keys pressed faster than a render are all applied", async () => {
	const setup = await testRender(<App client={client} index={index} store={store} />, { width: 140, height: 20 });
	try {
		setup.mockInput.pressKey("j");
		setup.mockInput.pressKey("j");
		await settle(setup);
		// The preview pane's title is the selected chat.
		expect(setup.captureCharFrame()).toContain("─Unjudged third chat");

		setup.mockInput.pressKey("?");
		setup.mockInput.pressKey("x"); // closes help
		setup.mockInput.pressKey("2"); // must reach the list, not the closed help
		await settle(setup);
		expect(setup.captureCharFrame()).toContain("1 of 3 · active · delete");
	} finally {
		setup.renderer.destroy();
	}
});

test("help shows every line", async () => {
	const setup = await testRender(<App client={client} index={index} store={store} />, { width: 140, height: 24 });
	try {
		setup.mockInput.pressKey("?");
		await settle(setup);
		const frame = setup.captureCharFrame();
		expect(frame).toContain("j/k");
		expect(frame).toContain("quit");
	} finally {
		setup.renderer.destroy();
	}
});

test("edits a local title without changing the ChatGPT title", async () => {
	const setup = await testRender(<App client={client} index={index} store={store} />, { width: 140, height: 20 });
	try {
		setup.mockInput.pressKey("n");
		await settle(setup);
		expect(setup.captureCharFrame()).toContain("Edit local title");
		setup.mockInput.pressKey("a", { ctrl: true });
		setup.mockInput.pressKey("k", { ctrl: true });
		await setup.mockInput.typeText("Better chat title");
		await settle(setup);
		setup.mockInput.pressEnter();
		await settle(setup);
		expect(setup.captureCharFrame()).toContain("Better chat title");
		expect(index.get("a1")[0]?.title).toBe("Empty test chat");
		expect(index.get("a1")[0]?.local_title).toBe("Better chat title");
	} finally {
		setup.renderer.destroy();
	}
});
