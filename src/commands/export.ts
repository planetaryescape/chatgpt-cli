import { writeFileSync } from "node:fs";
import type { ChatGPTClient } from "../api/client.ts";
import { getConversation } from "../api/conversations.ts";
import type { ConversationIndex } from "../index/store.ts";
import { renderTranscript } from "../render/transcript.ts";
import { selectOne, type SelectOptions } from "./select.ts";
import { note } from "../progress.ts";

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;

// Accepts a chat link (including project links, /g/g-p-…/c/<id>), a full id,
// or an id prefix from the local index. Links and full ids need no sync.
async function resolveId(index: ConversationIndex, ref: string, opts: SelectOptions): Promise<string> {
	if (/\/share\//.test(ref)) throw new Error("Shared links (/share/…) aren't supported; use the chat's own /c/… link.");
	const id = UUID.exec(ref)?.[0];
	if (id) return id.toLowerCase();
	return (await selectOne(index, ref, opts)).id;
}

function slugify(title: string): string {
	return title.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 80) || "conversation";
}

export async function copyToClipboard(text: string) {
	if (process.platform !== "darwin") throw new Error("--copy uses pbcopy and only works on macOS.");
	const proc = Bun.spawn(["pbcopy"], { stdin: "pipe" });
	proc.stdin.write(text);
	proc.stdin.end();
	if ((await proc.exited) !== 0) throw new Error("pbcopy failed.");
}

export async function exportConversation(
	client: ChatGPTClient,
	index: ConversationIndex,
	ref: string,
	opts: { output?: string | boolean; copy?: boolean } & SelectOptions,
) {
	const convo = await getConversation(client, await resolveId(index, ref, opts));
	if (!opts.all && Boolean(convo.is_archived) !== Boolean(opts.archived)) {
		throw new Error(`Conversation is ${convo.is_archived ? "archived; pass --archived or --all to include it" : "active; omit --archived or pass --all to include it"}.`);
	}
	const markdown = renderTranscript(convo);
	const kb = `${Math.round(Buffer.byteLength(markdown) / 1024)} KB`;
	if (opts.output) {
		const path = typeof opts.output === "string" ? opts.output : `${slugify(convo.title)}.md`;
		writeFileSync(path, markdown);
		note(`wrote ${path} (${kb})`);
	}
	if (opts.copy) {
		await copyToClipboard(markdown);
		note(`copied "${convo.title}" to the clipboard (${kb})`);
	}
	if (!opts.output && !opts.copy) process.stdout.write(markdown);
}
