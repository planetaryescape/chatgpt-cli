import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ApiError, type ChatGPTClient } from "../api/client.ts";
import { QUESTIONS_VERSION } from "../classify/questions.ts";
import { DEEP_QUESTIONS_VERSION } from "../classify/deep-questions.ts";
import { LUNA_JUDGMENT_VERSION } from "../classify/luna-version.ts";
import { ClassificationStore } from "../index/classification-store.ts";
import { ConversationIndex } from "../index/store.ts";
import { applyAction, performAction } from "./mutate.ts";

const dirs: string[] = [];
afterEach(() => {
	for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function fixture(ids: string[]) {
	const dir = mkdtempSync(join(tmpdir(), "chatgpt-delete-test-"));
	dirs.push(dir);
	mkdirSync(join(dir, "chatgpt-cli"));
	const index = new ConversationIndex(join(dir, "chatgpt-cli/index.db"));
	const updateTime = "2026-09-01T00:00:00Z";
	index.replaceAll(ids.map((id) => ({
		id, title: id, create_time: updateTime, update_time: updateTime,
		is_archived: false, pinned_time: null, gizmo_id: null,
	})));
	return { dir, index, updateTime };
}

function answers(personalRecord: number) {
	return JSON.stringify({
		worth_keeping: { type: "score", score: 1, confidence: 0.9 },
		nothing_there: { type: "noul", noul: 0.02 },
		re_askable: { type: "noul", noul: 0.9 },
		unfinished: { type: "noul", noul: 0.05 },
		personal_record: { type: "noul", noul: personalRecord },
		brainstorming: { type: "noul", noul: 0.02 },
		brainstorm_for: { type: "choice", choice: "none", confidence: 0.9 },
		topic: { type: "choice", choice: "other", confidence: 0.9 },
	});
}

test("delete recommendations hold back unsure chats without --check", () => {
	const { dir, index, updateTime } = fixture(["sure", "unsure"]);
	const store = new ClassificationStore(index.db);
	for (const [id, personal] of [["sure", 0.1], ["unsure", 0.45]] as const) {
		const time = id === "unsure" ? "2026-09-02T00:00:00Z" : updateTime;
		if (id === "unsure") index.db.run("update conversations set update_time = ? where id = ?", [time, id]);
		store.saveJudgment({ id, update_time: time, version: QUESTIONS_VERSION, content_kind: "full", answers: answers(personal), classified_at: time });
	}
	const proc = Bun.spawnSync(["bun", join(import.meta.dir, "../cli.ts"), "delete", "--suggest", "delete", "--limit", "1", "--dry-run"], {
		env: { ...process.env, XDG_DATA_HOME: dir },
	});
	const output = proc.stderr.toString();
	expect(proc.exitCode).toBe(0);
	expect(output).toContain("Classification backs delete for 1 of 1.");
	expect(output).not.toContain("delete?     unsure");
	expect(output).toContain("dry run: would delete 1 conversation(s).");
});

test("archive recommendations hold back unsure chats before --limit", () => {
	const { dir, index, updateTime } = fixture(["sure", "unsure"]);
	const store = new ClassificationStore(index.db);
	for (const [id, personal] of [["sure", 0.1], ["unsure", 0.45]] as const) {
		const time = id === "unsure" ? "2026-09-02T00:00:00Z" : updateTime;
		if (id === "unsure") index.db.run("update conversations set update_time = ? where id = ?", [time, id]);
		const a = JSON.parse(answers(personal));
		a.re_askable.noul = 0.05;
		store.saveJudgment({ id, update_time: time, version: QUESTIONS_VERSION, content_kind: "full", answers: JSON.stringify(a), classified_at: time });
	}
	const proc = Bun.spawnSync(["bun", join(import.meta.dir, "../cli.ts"), "archive", "--suggest", "archive", "--limit", "1", "--dry-run"], {
		env: { ...process.env, XDG_DATA_HOME: dir },
	});
	const output = proc.stderr.toString();
	expect(proc.exitCode).toBe(0);
	expect(output).toContain("Classification backs archive for 1 of 1.");
	expect(output).toContain("dry run: would archive 1 conversation(s).");
});

test("a Luna-resolved delete is eligible for a dry run", () => {
	const { dir, index, updateTime } = fixture(["resolved"]);
	const store = new ClassificationStore(index.db);
	store.saveJudgment({ id: "resolved", update_time: updateTime, version: QUESTIONS_VERSION, content_kind: "full",
		answers: answers(0.45), classified_at: updateTime });
	store.saveDeepJudgment({ id: "resolved", update_time: updateTime, questions_version: QUESTIONS_VERSION,
		version: DEEP_QUESTIONS_VERSION, classified_at: updateTime,
		answers: JSON.stringify({ personal_record_lost: { noul: 0.4 }, reusable_artifact_lost: { noul: 0.3 },
			original_thinking_lost: { noul: 0.2 }, work_to_resume: { noul: 0.3 }, creative_idea_lost: { noul: 0.1 },
			reaskable_without_loss: { noul: 0.6 }, worth_finding_again: { score: 1.5, confidence: 0.3 } }) });
	store.saveLunaJudgment({ id: "resolved", update_time: updateTime, questions_version: QUESTIONS_VERSION,
		deep_version: DEEP_QUESTIONS_VERSION, version: LUNA_JUDGMENT_VERSION, suggestion: "delete",
		brainstorm: null, reason: "Only expired one-off help" });
	const proc = Bun.spawnSync(["bun", join(import.meta.dir, "../cli.ts"), "delete", "--suggest", "delete", "--dry-run"], {
		env: { ...process.env, XDG_DATA_HOME: dir },
	});
	expect(proc.exitCode).toBe(0);
	expect(proc.stderr.toString()).toContain("dry run: would delete 1 conversation(s).");
});

test("delete uses at most three concurrent requests", async () => {
	const { index } = fixture(["a", "b", "c", "d", "e", "f"]);
	let active = 0;
	let peak = 0;
	const client = {
		request: async () => {
			peak = Math.max(peak, ++active);
			await Bun.sleep(20);
			active--;
			return null;
		},
	} as unknown as ChatGPTClient;
	await applyAction(client, index, "delete", index.query({ includePinned: true }), { yes: true });
	expect(peak).toBe(3);
	expect(index.query({ includePinned: true })).toHaveLength(0);
});

test("an already deleted chat is removed from the stale index", async () => {
	const { index } = fixture(["gone"]);
	const client = {
		request: async () => { throw new ApiError(404, "/backend-api/conversation/id/gone", "conversation_deleted"); },
	} as unknown as ChatGPTClient;
	await performAction(client, index, "delete", "gone");
	expect(index.get("gone")).toHaveLength(0);
});
