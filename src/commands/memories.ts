import type { ChatGPTClient } from "../api/client.ts";
import { deleteMemory, listMemories, type MemorySummary, type SavedMemory } from "../api/memories.ts";
import { note } from "../progress.ts";
import { ask } from "./mutate.ts";
import type { ClassifiedMemory, MemorySuggestion } from "../classify/memories.ts";

export type MemoryFormat = "json" | "csv" | "table" | "ids";

function csvCell(value: string): string {
	return /[",\r\n]/.test(value) ? `"${value.replaceAll('"', '""')}"` : value;
}

function preview(content: string): string {
	const singleLine = content.replace(/\s+/g, " ").trim();
	return singleLine.length > 120 ? `${singleLine.slice(0, 119)}…` : singleLine;
}

export function formatMemories(memories: SavedMemory[], format: MemoryFormat): string {
	if (format === "json") return JSON.stringify(memories, null, 2);
	if (format === "ids") return memories.map((memory) => memory.id).join("\n");
	if (format === "csv") {
		return ["id,content,updated_at,status,conversation_id", ...memories.map((m) =>
			[m.id, m.content, m.updated_at, m.status, m.conversation_id ?? ""].map(csvCell).join(","))].join("\n");
	}
	return ["ID                                    UPDATED      CONTENT", ...memories.map((m) =>
		`${m.id}  ${m.updated_at.slice(0, 10)}  ${preview(m.content)}`)].join("\n");
}

export function formatClassifiedMemories(memories: ClassifiedMemory[], format: MemoryFormat): string {
	if (format === "json") return JSON.stringify(memories, null, 2);
	if (format === "ids") return memories.map((m) => m.id).join("\n");
	if (format === "csv") return ["id,suggestion,stage,reason,content,updated_at,related_ids", ...memories.map((m) =>
		[m.id, m.suggestion, m.stage, m.reason, m.content, m.updated_at, m.related_ids.join(" ")].map(csvCell).join(","))].join("\n");
	return ["SUGGEST  STAGE  ID                                    REASON", ...memories.map((m) =>
		`${m.suggestion.padEnd(7)}  ${m.stage.padEnd(5)}  ${m.id}  ${m.reason}  [${preview(m.content)}]`)].join("\n");
}

export function selectClassifiedMemories(rows: ClassifiedMemory[], suggestion?: MemorySuggestion): ClassifiedMemory[] {
	return suggestion ? rows.filter((m) => m.suggestion === suggestion) : rows;
}

export function formatMemorySummary(summary: MemorySummary, format: "json" | "table"): string {
	if (format === "json") return JSON.stringify(summary, null, 2);
	if (!summary.sections.length) return summary.emptyStateMessage ?? "No memory summary available.";
	return summary.sections.map((section) => `${section.title}\n${section.description}`).join("\n\n");
}

export async function writeData(output: string): Promise<void> {
	if (!output) return;
	await new Promise<void>((resolve, reject) => {
		process.stdout.write(`${output}\n`, (error) => error ? reject(error) : resolve());
	});
}

export function resolveMemories(memories: SavedMemory[], ids: string[]): SavedMemory[] {
	const selected = new Map<string, SavedMemory>();
	for (const raw of ids) {
		const id = raw.trim().toLowerCase();
		if (!id) throw new Error("Saved memory id cannot be empty.");
		const matches = memories.filter((memory) => memory.id.toLowerCase().startsWith(id));
		if (!matches.length) throw new Error(`No saved memory matching "${raw}". Run \`chatgpt memory list\` to see current ids.`);
		if (matches.length > 1) throw new Error(`"${raw}" matches ${matches.length} saved memories; use a longer id prefix.`);
		const memory = matches[0] as SavedMemory;
		selected.set(memory.id, memory);
	}
	return [...selected.values()];
}

export async function deleteSelectedMemories(
	client: ChatGPTClient,
	ids: string[],
	opts: { dryRun?: boolean; yes?: boolean },
): Promise<void> {
	const raw = ids.length === 1 && ids[0] === "-"
		? (await Bun.stdin.text()).split("\n").map((line) => line.trim().split(/\s+/)[0] ?? "").filter(Boolean)
		: ids;
	if (!raw.length) throw new Error("Pass saved memory ids, or `-` to read ids from stdin.");
	const targets = resolveMemories(await listMemories(client), raw);
	for (const memory of targets.slice(0, 25)) note(`${memory.id}  ${preview(memory.content)}`);
	if (targets.length > 25) note(`… and ${targets.length - 25} more`);
	if (opts.dryRun) {
		note(`dry run: would delete ${targets.length} saved memor${targets.length === 1 ? "y" : "ies"}.`);
		return;
	}
	if (!opts.yes && await ask(`Permanently delete ${targets.length} saved memor${targets.length === 1 ? "y" : "ies"}? Type ${targets.length} to confirm: `) !== String(targets.length)) {
		note("Cancelled.");
		return;
	}
	let deleted = 0;
	const failures: string[] = [];
	for (const [i, memory] of targets.entries()) {
		try {
			await deleteMemory(client, memory.id);
			deleted++;
		} catch (err) {
			failures.push(`${memory.id}: ${err instanceof Error ? err.message : String(err)}`);
		}
		if (i + 1 < targets.length) await Bun.sleep(250);
	}
	note(`Deleted ${deleted} saved memor${deleted === 1 ? "y" : "ies"}${failures.length ? `, ${failures.length} failed` : ""}.`);
	for (const failure of failures) console.error(`failed: ${failure}`);
	if (failures.length) process.exitCode = 1;
}
