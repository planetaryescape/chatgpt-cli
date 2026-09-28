import type { ChatGPTClient } from "../api/client.ts";
import { addConversationToProject, removeConversationFromProject, type Project } from "../api/projects.ts";
import { displayTitle, type ConversationIndex, type IndexedConversation } from "../index/store.ts";
import { note, Step } from "../progress.ts";
import { ask } from "./mutate.ts";

const PREVIEW_ROWS = 25;
const DELAY_MS = 250;

export function resolveProject(projects: Project[], reference: string): Project {
	const input = reference.trim().toLocaleLowerCase();
	const exact = projects.filter((project) => project.id.toLocaleLowerCase() === input || project.name.toLocaleLowerCase() === input);
	const matches = exact.length ? exact : projects.filter((project) => project.id.toLocaleLowerCase().startsWith(input));
	if (!matches.length) throw new Error(`No project matching "${reference}". Run \`chatgpt project list\` to see names and ids.`);
	if (matches.length > 1) throw new Error(`"${reference}" matches ${matches.length} projects; use a project id.`);
	return matches[0] as Project;
}

export async function applyProjectAdd(
	client: ChatGPTClient,
	index: ConversationIndex,
	project: Project,
	targets: IndexedConversation[],
	opts: { dryRun?: boolean; yes?: boolean },
): Promise<void> {
	if (!project.canWrite) throw new Error(`You do not have write access to project "${project.name}".`);
	const unique = [...new Map(targets.map((chat) => [chat.id, chat])).values()];
	if (!unique.length) {
		note("No chats to move.");
		return;
	}
	const pending = unique.filter((chat) => chat.project_id !== project.id);
	if (!pending.length) {
		note(`No chats to move; all ${unique.length} are already in "${project.name}".`);
		return;
	}
	for (const chat of pending.slice(0, PREVIEW_ROWS)) note(`${chat.id}  ${displayTitle(chat)}${chat.project_id ? `  (from ${chat.project_id})` : ""}`);
	if (pending.length > PREVIEW_ROWS) note(`… and ${pending.length - PREVIEW_ROWS} more`);
	if (opts.dryRun) {
		note(`dry run: would move ${pending.length} chat(s) to "${project.name}".`);
		return;
	}
	if (!opts.yes && !/^y(es)?$/i.test(await ask(`Move ${pending.length} chat(s) to "${project.name}"? [y/N] `))) {
		note("Cancelled.");
		return;
	}

	const failures: string[] = [];
	let moved = 0;
	const step = new Step("Moving chats to project", pending.length);
	for (const [i, chat] of pending.entries()) {
		try {
			await addConversationToProject(client, chat.id, project.id);
			index.setProject(chat.id, project.id);
			moved++;
		} catch (err) {
			failures.push(`${chat.id} ${chat.title}: ${err instanceof Error ? err.message : String(err)}`);
		}
		step.update(i + 1, failures.length ? `${failures.length} failed` : "");
		if (i + 1 < pending.length) await Bun.sleep(DELAY_MS);
	}
	step.finish(`Moved ${moved} chat(s) to "${project.name}"${failures.length ? `, ${failures.length} failed` : ""}`);
	for (const failure of failures) console.error(`failed: ${failure}`);
	if (failures.length) process.exitCode = 1;
}

export async function applyProjectRemove(
	client: ChatGPTClient,
	index: ConversationIndex,
	project: Project,
	targets: IndexedConversation[],
	opts: { dryRun?: boolean; yes?: boolean },
): Promise<void> {
	if (!project.canWrite) throw new Error(`You do not have write access to project "${project.name}".`);
	const unique = [...new Map(targets.map((chat) => [chat.id, chat])).values()];
	const pending = unique.filter((chat) => chat.project_id === project.id);
	if (!pending.length) {
		note(`No selected chats are in "${project.name}".`);
		return;
	}
	for (const chat of pending.slice(0, PREVIEW_ROWS)) note(`${chat.id}  ${displayTitle(chat)}`);
	if (pending.length > PREVIEW_ROWS) note(`… and ${pending.length - PREVIEW_ROWS} more`);
	if (opts.dryRun) {
		note(`dry run: would remove ${pending.length} chat(s) from "${project.name}".`);
		return;
	}
	if (!opts.yes && !/^y(es)?$/i.test(await ask(`Remove ${pending.length} chat(s) from "${project.name}"? [y/N] `))) {
		note("Cancelled.");
		return;
	}

	const failures: string[] = [];
	let removed = 0;
	const step = new Step("Removing chats from project", pending.length);
	for (const [i, chat] of pending.entries()) {
		try {
			await removeConversationFromProject(client, chat.id);
			index.setProject(chat.id, null);
			removed++;
		} catch (err) {
			failures.push(`${chat.id} ${chat.title}: ${err instanceof Error ? err.message : String(err)}`);
		}
		step.update(i + 1, failures.length ? `${failures.length} failed` : "");
		if (i + 1 < pending.length) await Bun.sleep(DELAY_MS);
	}
	step.finish(`Removed ${removed} chat(s) from "${project.name}"${failures.length ? `, ${failures.length} failed` : ""}`);
	for (const failure of failures) console.error(`failed: ${failure}`);
	if (failures.length) process.exitCode = 1;
}
