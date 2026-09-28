import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ConversationIndex } from "../index/store.ts";

test("large list JSON stays complete when piped, and ids output has one id per line", () => {
	const dir = mkdtempSync(join(tmpdir(), "chatgpt-list-output-"));
	try {
		const dataDir = join(dir, "chatgpt-cli");
		mkdirSync(dataDir);
		const index = new ConversationIndex(join(dataDir, "index.db"));
		const chats = Array.from({ length: 150 }, (_, i) => ({
			id: `chat-${String(i).padStart(3, "0")}`,
			title: `Chat ${i} ${"a".repeat(700)}`,
			create_time: "2026-09-28T00:00:00Z",
			update_time: "2026-09-28T00:00:00Z",
			is_archived: false,
			pinned_time: null,
			gizmo_id: null,
		}));
		index.replaceAll(chats);
		index.db.close();
		const run = (format: string[]) => Bun.spawnSync(["bun", join(import.meta.dir, "../cli.ts"), "list", ...format], {
			env: { ...process.env, XDG_DATA_HOME: dir },
		});
		const json = run(["--json"]);
		expect(json.exitCode).toBe(0);
		expect((JSON.parse(json.stdout.toString()) as unknown[]).length).toBe(chats.length);
		const ids = run(["--format", "ids"]);
		expect(ids.exitCode).toBe(0);
		expect(ids.stdout.toString().trimEnd().split("\n")).toHaveLength(chats.length);
		expect(ids.stdout.toString()).toContain("chat-000\n");
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
});
