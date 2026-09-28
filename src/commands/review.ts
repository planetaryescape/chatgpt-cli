import { openSync } from "node:fs";
import { ReadStream } from "node:tty";
import type { ChatGPTClient } from "../api/client.ts";
import { type Conversation, getConversationsBatch } from "../api/conversations.ts";
import { topicOf, type Verdict, verdictOf } from "../classify/policy.ts";
import { QUESTIONS_VERSION } from "../classify/questions.ts";
import { SUMMARY_PROMPT_VERSION } from "../classify/summarise.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import { displayTitle, type ConversationIndex, type IndexedConversation } from "../index/store.ts";
import { renderTranscript, visibleTurns } from "../render/transcript.ts";
import { applyAction, ask } from "./mutate.ts";
import { note } from "../progress.ts";

type Decision = "keep" | "archive" | "delete";

const HELP = "[k]eep  [a]rchive  [d]elete  [v]iew  [o]pen in browser  [u]ndo  [q]uit";
const HELP_WITH_JEV = `[enter] accept suggestion  ${HELP}`;

// Opened on /dev/tty so review works even when ids were piped on stdin.
async function readKey(tty: ReadStream): Promise<string> {
	return new Promise((resolve) => tty.once("data", (buf: Buffer) => resolve(buf.toString())));
}

// The batch endpoint avoids the single-fetch rate limit that would stall review.
async function load(client: ChatGPTClient, id: string): Promise<Conversation> {
	const [convo] = await getConversationsBatch(client, [id]);
	if (!convo) throw new Error("ChatGPT didn't return this conversation (deleted?)");
	return convo;
}

function clip(text: string, max: number): string {
	const oneLine = text.replace(/\s+/g, " ").trim();
	return oneLine.length > max ? `${oneLine.slice(0, max)}…` : oneLine;
}

type JevView = { verdict: Verdict; topic: string; summary: string | null };

async function preview(client: ChatGPTClient, c: IndexedConversation, position: string, jev: JevView | undefined) {
	console.clear();
	console.log(`${position}  ${displayTitle(c)}`);
	console.log(`updated ${c.update_time.slice(0, 10)} · created ${c.create_time.slice(0, 10)}${c.project_id ? " · in a project" : ""}${c.is_archived ? " · archived" : ""}\n`);
	if (jev) {
		const { suggestion, unsure, reason } = jev.verdict;
		console.log(`${jev.verdict.luna ? "Luna" : "Jev"} suggests: ${suggestion.toUpperCase()}${unsure ? " (unsure)" : ""} · ${jev.topic}\n  ${reason}\n`);
		// Long chats were judged from a summary; show it, it's the best overview.
		if (jev.summary) console.log(`Summary: ${clip(jev.summary, 900)}\n`);
	}
	try {
		const turns = visibleTurns(await load(client, c.id));
		const firstUser = turns.find((t) => t.role === "user");
		const lastAssistant = turns.findLast((t) => t.role === "assistant");
		console.log(`${turns.length} turns`);
		if (firstUser) console.log(`\nYou: ${clip(firstUser.text, 500)}`);
		if (lastAssistant) console.log(`\nChatGPT (last): ${clip(lastAssistant.text, 400)}`);
	} catch (err) {
		console.log(`(could not load: ${err instanceof Error ? err.message : String(err)})`);
	}
	console.log(`\n${jev ? HELP_WITH_JEV : HELP}`);
}

async function page(text: string) {
	const pager = Bun.spawn([process.env.PAGER ?? "less", "-R"], { stdin: "pipe", stdout: "inherit", stderr: "inherit" });
	pager.stdin.write(text);
	pager.stdin.end();
	await pager.exited;
}

export async function review(
	client: ChatGPTClient,
	index: ConversationIndex,
	store: ClassificationStore,
	targets: IndexedConversation[],
) {
	if (targets.length === 0) {
		note("Nothing matched.");
		return;
	}
	const judgments = store.currentJudgments(QUESTIONS_VERSION);
	const jevFor = (c: IndexedConversation): JevView | undefined => {
		const j = judgments.get(c.id);
		if (!j) return undefined;
		const summary = j.content_kind === "summary" ? (store.summary(c.id, c.update_time, SUMMARY_PROMPT_VERSION)?.summary ?? null) : null;
		return { verdict: verdictOf(j), topic: topicOf(j), summary };
	};
	const tty = new ReadStream(openSync("/dev/tty", "r"));
	const decisions = new Map<string, Decision>();
	let i = 0;
	try {
		tty.setRawMode(true);
		while (i < targets.length) {
			const c = targets[i] as IndexedConversation;
			const jev = jevFor(c);
			await preview(client, c, `[${i + 1}/${targets.length}]`, jev);
			const key = await readKey(tty);
			if (key === "q" || key === "\u0003") break;
			if (key === "u") {
				i = Math.max(0, i - 1);
				decisions.delete((targets[i] as IndexedConversation).id);
				continue;
			}
			if (key === "v") {
				tty.setRawMode(false);
				await page(renderTranscript(await load(client, c.id)));
				tty.setRawMode(true);
				continue;
			}
			if (key === "o") {
				Bun.spawn(["open", `https://chatgpt.com/c/${c.id}`]);
				continue;
			}
			const accepted = (key === "\r" || key === "\n") && jev ? jev.verdict.suggestion : undefined;
			const decision: Decision | undefined =
				accepted ?? (key === "k" || key === " " ? "keep" : key === "a" ? "archive" : key === "d" ? "delete" : undefined);
			if (!decision) continue;
			decisions.set(c.id, decision);
			i++;
		}
	} finally {
		tty.setRawMode(false);
		tty.destroy();
	}

	console.clear();
	const pick = (d: Decision) => targets.filter((t) => decisions.get(t.id) === d);
	const toArchive = pick("archive");
	const toDelete = pick("delete");
	note(`Reviewed ${decisions.size}: keep ${pick("keep").length}, archive ${toArchive.length}, delete ${toDelete.length}.`);
	if (toArchive.length + toDelete.length === 0) return;
	for (const c of toDelete) note(`delete   ${displayTitle(c)}`);
	for (const c of toArchive) note(`archive  ${displayTitle(c)}`);
	if ((await ask("Type apply to carry these out: ")) !== "apply") {
		note("Nothing changed.");
		return;
	}
	await applyAction(client, index, "archive", toArchive, { yes: true });
	await applyAction(client, index, "delete", toDelete, { yes: true });
}
