import { expect, test } from "bun:test";
import type { ChatGPTClient } from "../api/client.ts";
import type { Conversation, ConversationSummary } from "../api/conversations.ts";
import { QUESTIONS_VERSION } from "../classify/questions.ts";
import { LUNA_JUDGMENT_VERSION } from "../classify/luna-version.ts";
import { RENDER_VERSION, toCachedTranscript } from "../render/transcript.ts";
import { SearchStore } from "../search/store.ts";
import { ClassificationStore } from "./classification-store.ts";
import { reconcileMetadataChanges } from "./reconcile-metadata.ts";
import { ConversationIndex } from "./store.ts";

const id = "chat-one";
const oldTime = "2026-09-01T00:00:00.000Z";
const newTime = "2026-09-28T00:00:00.000Z";
const created = "2026-09-01T00:00:00.000Z";

function conversation(text: string, title = "Idea"): Conversation {
	return {
		conversation_id: id,
		title,
		create_time: Date.parse(created) / 1000,
		update_time: Date.parse(oldTime) / 1000,
		default_model_slug: "gpt-4",
		mapping: { message: {
			id: "message", parent: null, children: [],
			message: { author: { role: "user" }, create_time: Date.parse(created) / 1000, content: { content_type: "text", parts: [text] } },
		} },
		current_node: "message",
	};
}

function setup() {
	const index = new ConversationIndex(":memory:");
	const classifications = new ClassificationStore(index.db);
	const search = new SearchStore(index.db);
	const summary: ConversationSummary = {
		id, title: "Idea", create_time: created, update_time: oldTime,
		is_archived: false, pinned_time: null, gizmo_id: null,
	};
	index.replaceAll([summary]);
	const cached = toCachedTranscript(id, oldTime, conversation("A product idea worth developing."));
	classifications.saveTranscript(cached);
	classifications.saveSummary({ id, update_time: oldTime, prompt_version: 1, summary: "An idea", model: "test" });
	classifications.saveJudgment({ id, update_time: oldTime, version: QUESTIONS_VERSION, content_kind: "full", answers: "{}", classified_at: oldTime });
	classifications.saveDeepJudgment({ id, update_time: oldTime, questions_version: QUESTIONS_VERSION, version: "test", answers: "{}", classified_at: oldTime });
	classifications.saveLunaJudgment({ id, update_time: oldTime, questions_version: QUESTIONS_VERSION, deep_version: "test",
		version: LUNA_JUDGMENT_VERSION, suggestion: "keep", brainstorm: "product", reason: "Own idea" });
	index.setLocalTitle(index.get(id)[0]!, "Specific product concept", "own product", "luna");
	search.replace(index.get(id)[0]!, cached.markdown);
	index.applyDelta([{ ...summary, update_time: newTime, gizmo_id: "g-p-project" }], []);
	return { index, classifications, search };
}

function clientWith(text: string, batchTime = newTime): ChatGPTClient {
	return { async request(_method: string, _path: string, _body: unknown) {
		const chat = conversation(text);
		return [{ ...chat, id, create_time: created, update_time: batchTime }];
	} } as ChatGPTClient;
}

test("metadata-only project moves preserve judgments, summaries, transcripts and search chunks", async () => {
	const { index, classifications, search } = setup();
	try {
		expect(search.isCurrent(index.get(id)[0]!)).toBe(false);
		const result = await reconcileMetadataChanges(clientWith("A product idea worth developing.", oldTime), index, [id]);
		expect(result).toEqual({ preserved: 1, changed: 0, failures: [] });
		expect(classifications.transcript(id, newTime, RENDER_VERSION)).not.toBeNull();
		expect(classifications.summary(id, newTime, 1)).not.toBeNull();
		expect(classifications.judgment(id, newTime, QUESTIONS_VERSION)).not.toBeNull();
		expect(classifications.deepJudgment(id, newTime, QUESTIONS_VERSION, "test")).not.toBeNull();
		expect(classifications.lunaJudgment(id, newTime, QUESTIONS_VERSION, "test", LUNA_JUDGMENT_VERSION)).toBe(true);
		expect(index.localTitle(index.get(id)[0]!)?.title).toBe("Specific product concept");
		expect(search.isCurrent(index.get(id)[0]!)).toBe(true);
	} finally {
		index.db.close();
	}
});

test("content edits keep the previous judgment and search chunks stale", async () => {
	const { index, classifications, search } = setup();
	try {
		const result = await reconcileMetadataChanges(clientWith("A changed idea."), index, [id]);
		expect(result).toEqual({ preserved: 0, changed: 1, failures: [] });
		expect(classifications.transcript(id, newTime, RENDER_VERSION)).toBeNull();
		expect(classifications.currentJudgments(QUESTIONS_VERSION).size).toBe(0);
		expect(index.localTitle(index.get(id)[0]!)).toBeNull();
		expect(search.isCurrent(index.get(id)[0]!)).toBe(false);
	} finally {
		index.db.close();
	}
});
