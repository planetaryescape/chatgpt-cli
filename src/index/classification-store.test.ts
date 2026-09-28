import { expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { DEEP_QUESTIONS_VERSION } from "../classify/deep-questions.ts";
import { QUESTIONS_VERSION } from "../classify/questions.ts";
import { ClassificationStore, type Judgment } from "./classification-store.ts";

const base: Judgment = {
	id: "chat-1",
	update_time: "2026-09-27T00:00:00Z",
	version: QUESTIONS_VERSION,
	content_kind: "full",
	answers: "{}",
	classified_at: "2026-09-27T01:00:00Z",
};

test("deep answers join only their current base judgment and are cleared on re-judgment", () => {
	const db = new Database(":memory:");
	try {
		db.run("create table conversations (id text primary key, update_time text not null)");
		db.query("insert into conversations values (?, ?)").run(base.id, base.update_time);
		const store = new ClassificationStore(db);
		store.saveJudgment(base);
		store.saveDeepJudgment({
			id: base.id,
			update_time: base.update_time,
			questions_version: QUESTIONS_VERSION,
			version: DEEP_QUESTIONS_VERSION,
			answers: "{\"followup\":true}",
			classified_at: "2026-09-27T02:00:00Z",
		});
		expect(store.judgment(base.id, base.update_time, QUESTIONS_VERSION)?.deep_answers).toBe("{\"followup\":true}");
		expect(store.currentJudgments(QUESTIONS_VERSION).get(base.id)?.deep_version).toBe(DEEP_QUESTIONS_VERSION);
		expect(store.judgment(base.id, "changed", QUESTIONS_VERSION)).toBeNull();
		store.saveJudgment({ ...base, classified_at: "2026-09-27T03:00:00Z" });
		expect(store.judgment(base.id, base.update_time, QUESTIONS_VERSION)?.deep_answers).toBeNull();
		expect(store.deepJudgment(base.id, base.update_time, QUESTIONS_VERSION, DEEP_QUESTIONS_VERSION)).toBeNull();
	} finally {
		db.close();
	}
});
