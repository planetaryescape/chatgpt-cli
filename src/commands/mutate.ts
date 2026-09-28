import { createReadStream } from "node:fs";
import { createInterface } from "node:readline/promises";
import { ApiError, type ChatGPTClient } from "../api/client.ts";
import { deleteConversation, setArchived } from "../api/conversations.ts";
import type { ConversationIndex, IndexedConversation } from "../index/store.ts";
import { note, Step } from "../progress.ts";
import { formatRow } from "./select.ts";

export type Action = "archive" | "unarchive" | "delete";

const PREVIEW_ROWS = 25;
// Gentle pacing: this is a private API on a personal account.
const DELAY_MS = 250;
const DELETE_CONCURRENCY = 3;

export async function ask(question: string): Promise<string> {
	// When ids are piped in, stdin is spent; the terminal is still at /dev/tty.
	const input = process.stdin.isTTY ? process.stdin : createReadStream("/dev/tty");
	const rl = createInterface({ input, output: process.stderr });
	try {
		return (await rl.question(question)).trim();
	} finally {
		rl.close();
		if (input !== process.stdin) input.destroy();
	}
}

async function confirm(action: Action, targets: IndexedConversation[]): Promise<boolean> {
	if (action === "delete") {
		const answer = await ask(`Permanently delete ${targets.length} conversation(s)? This cannot be undone. Type ${targets.length} to confirm: `);
		return answer === String(targets.length);
	}
	return /^y(es)?$/i.test(await ask(`${action} ${targets.length} conversation(s)? [y/N] `));
}

// One chat, upstream first, then the index. Shared by the CLI and the TUI.
export async function performAction(client: ChatGPTClient, index: ConversationIndex, action: Action, id: string) {
	try {
		if (action === "delete") await deleteConversation(client, id);
		else await setArchived(client, id, action === "archive");
	} catch (err) {
		// A repeated delete has reached its goal when the chat is already gone.
		if (err instanceof ApiError && err.status === 404) {
			index.remove(id);
			if (action === "delete") return;
		}
		throw err;
	}
	if (action === "delete") index.remove(id);
	else index.setArchived(id, action === "archive");
}

export async function applyAction(
	client: ChatGPTClient,
	index: ConversationIndex,
	action: Action,
	targets: IndexedConversation[],
	opts: { yes?: boolean; dryRun?: boolean },
) {
	if (targets.length === 0) {
		note("Nothing matched.");
		return;
	}
	for (const c of targets.slice(0, PREVIEW_ROWS)) note(formatRow(c));
	if (targets.length > PREVIEW_ROWS) note(`… and ${targets.length - PREVIEW_ROWS} more`);
	if (opts.dryRun) {
		note(`dry run: would ${action} ${targets.length} conversation(s).`);
		return;
	}
	if (!opts.yes && !(await confirm(action, targets))) {
		note("Cancelled.");
		return;
	}

	let done = 0;
	const failures: string[] = [];
	const [doing, did] = ({ archive: ["Archiving", "Archived"], unarchive: ["Unarchiving", "Unarchived"], delete: ["Deleting", "Deleted"] } as const)[action];
	const step = new Step(doing, targets.length);
	let next = 0;
	let attempted = 0;
	const worker = async () => {
		while (next < targets.length) {
			const c = targets[next++] as IndexedConversation;
			try {
				await performAction(client, index, action, c.id);
				done++;
			} catch (err) {
				failures.push(`${c.id} ${c.title}: ${err instanceof Error ? err.message : String(err)}`);
			}
			step.update(++attempted, failures.length ? `${failures.length} failed` : "");
			await Bun.sleep(DELAY_MS);
		}
	};
	await Promise.all(Array.from({ length: action === "delete" ? Math.min(DELETE_CONCURRENCY, targets.length) : 1 }, worker));
	step.finish(`${did} ${done} conversation(s)${failures.length ? `, ${failures.length} failed` : ""}`);
	for (const f of failures) console.error(`failed: ${f}`);
	if (failures.length) process.exitCode = 1;
}
