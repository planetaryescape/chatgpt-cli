import { TypeSafeClient } from "@typesafe-ai/sdk";
import type { ChatGPTClient } from "../api/client.ts";
import { BATCH_MAX, type Conversation, getConversationsBatch } from "../api/conversations.ts";
import { requireSecret } from "../auth/secrets.ts";
import { ask } from "../commands/mutate.ts";
import type { CachedSummary, CachedTranscript, ClassificationStore, Judgment } from "../index/classification-store.ts";
import type { IndexedConversation } from "../index/store.ts";
import { note, Step } from "../progress.ts";
import { RENDER_VERSION, toCachedTranscript } from "../render/transcript.ts";
import { QUESTIONS, QUESTIONS_VERSION } from "./questions.ts";
import { needsTimeRefresh } from "./policy.ts";
import { CostMeter, formatUsd, PRICES, priceOf } from "./costs.ts";
import { SUMMARY_PROMPT_VERSION, summarise, summariserAvailable, summariserNames } from "./summarise.ts";

// Jev takes 32k tokens of state; its accuracy drops as state grows, so chats
// above this get an LLM summary instead of a clipped transcript.
export const FULL_TRANSCRIPT_MAX_TOKENS = 12_000;
// Single-chat fetches hit ChatGPT's 429s within minutes; the batch endpoint
// doesn't at this pace. The gap is courtesy, not a measured limit.
const BATCH_GAP_MS = 500;
const CONCURRENCY = 4;
// Summaries use the user's Claude subscription; big batches eat into its usage limits.
const CONFIRM_ABOVE_TOKENS = 500_000;

export type ClassifyResult = { judgments: Map<string, Judgment>; failures: string[]; heldBack: IndexedConversation[] };


export class Classifier {
	private jev: TypeSafeClient | undefined;
	private meter = new CostMeter();

	constructor(
		private readonly chatgpt: ChatGPTClient,
		private readonly store: ClassificationStore,
	) {}

	private jevClient(): TypeSafeClient {
		this.jev ??= new TypeSafeClient({ apiKey: requireSecret("TYPESAFE_API_KEY") });
		return this.jev;
	}

	private saveTranscript(c: IndexedConversation, convo: Conversation): CachedTranscript {
		const transcript = toCachedTranscript(c.id, c.update_time, convo);
		this.store.saveTranscript(transcript);
		return transcript;
	}

	private async ensureSummary(c: IndexedConversation, t: CachedTranscript): Promise<CachedSummary> {
		const cached = this.store.summary(c.id, c.update_time, SUMMARY_PROMPT_VERSION);
		if (cached) return cached;
		const { summary, model, usd, tokens } = await summarise(c.title, t.markdown);
		this.meter.add(`summaries (${model})`, "subscription", usd, tokens);
		const fresh = { id: c.id, update_time: c.update_time, prompt_version: SUMMARY_PROMPT_VERSION, summary, model };
		this.store.saveSummary(fresh);
		return fresh;
	}

	private async judge(c: IndexedConversation, t: CachedTranscript): Promise<Judgment> {
		let content = t.markdown;
		let contentKind: Judgment["content_kind"] = "full";
		if (t.approx_tokens > FULL_TRANSCRIPT_MAX_TOKENS) {
			content = (await this.ensureSummary(c, t)).summary;
			contentKind = "summary";
		}
		const result = await this.jevClient().systemOne({
			state: {
				conversation: {
					title: c.title,
					as_of: new Date().toISOString().slice(0, 10),
					created: c.create_time.slice(0, 10),
					last_updated: c.update_time.slice(0, 10),
					turns: t.turns,
					in_a_project: Boolean(c.project_id),
					pinned: Boolean(c.pinned),
				},
				content_kind:
					contentKind === "full" ? "full transcript" : `summary of a long conversation (${t.turns} turns), written by another model`,
				content,
			},
			questions: QUESTIONS,
		});
		const jevTokens = result.usage.input_tokens;
		this.meter.add("Jev", "api", priceOf("jev", { inputTokens: jevTokens, cachedInputTokens: 0, outputTokens: 0 }), jevTokens);
		const judgment: Judgment = {
			id: c.id,
			update_time: c.update_time,
			version: QUESTIONS_VERSION,
			content_kind: contentKind,
			answers: JSON.stringify(result.answers),
			classified_at: new Date().toISOString(),
		};
		this.store.saveJudgment(judgment);
		return judgment;
	}

	async classify(targets: IndexedConversation[], opts: { force?: boolean; yes?: boolean } = {}): Promise<ClassifyResult> {
		this.meter = new CostMeter();
		const judgments = new Map<string, Judgment>();
		const failures: string[] = [];
		const heldBack: IndexedConversation[] = [];
		const todo: IndexedConversation[] = [];
		for (const c of targets) {
			const cached = opts.force ? null : this.store.judgment(c.id, c.update_time, QUESTIONS_VERSION);
			// A finite need can become obsolete while the chat itself stays unchanged.
			const dueForTimeReview = Boolean(cached && needsTimeRefresh(cached, new Date().toISOString().slice(0, 10)));
			const existing = dueForTimeReview ? null : cached;
			if (existing) judgments.set(c.id, existing);
			else todo.push(c);
		}
		const fail = (c: IndexedConversation, err: unknown) =>
			failures.push(`${c.id} ${c.title}: ${err instanceof Error ? err.message : String(err)}`);

		// Plan: what's already done, what's left.
		const transcripts = new Map<string, CachedTranscript>();
		const toFetch = todo.filter((c) => {
			const cached = this.store.transcript(c.id, c.update_time, RENDER_VERSION);
			if (cached) transcripts.set(c.id, cached);
			return !cached;
		});
		note(
			opts.force
				? `Re-judging all ${targets.length} matching chat(s).`
				: `${targets.length} chat(s): ${judgments.size} already judged, ${todo.length} new or changed to judge.`,
		);
		if (todo.length === 0) return { judgments, failures, heldBack };
		note("Steps: [1/3] download transcripts → [2/3] judge short chats → [3/3] summarise and judge long chats");

		// Download, in batches. Saved batch by batch, so an interrupted run resumes.
		if (!toFetch.length) note(`[1/3] Download transcripts: all ${transcripts.size} cached, nothing to download.`);
		else {
			const step = new Step("[1/3] Downloading transcripts", toFetch.length);
			let downloaded = 0;
			for (let i = 0; i < toFetch.length; i += BATCH_MAX) {
				const batch = toFetch.slice(i, i + BATCH_MAX);
				try {
					const byId = new Map((await getConversationsBatch(this.chatgpt, batch.map((c) => c.id))).map((cv) => [cv.conversation_id, cv]));
					for (const c of batch) {
						const convo = byId.get(c.id);
						if (convo) {
							transcripts.set(c.id, this.saveTranscript(c, convo));
							downloaded++;
						} else fail(c, new Error("not returned by ChatGPT (deleted? run `chatgpt sync`)"));
					}
				} catch (err) {
					for (const c of batch) fail(c, err);
				}
				step.update(Math.min(i + BATCH_MAX, toFetch.length));
				if (i + BATCH_MAX < toFetch.length) await Bun.sleep(BATCH_GAP_MS);
			}
			step.finish(`Downloaded ${downloaded} transcript(s)${downloaded < toFetch.length ? `, ${toFetch.length - downloaded} failed` : ""}`);
		}

		// Short chats first: they need no summary, so results land in seconds.
		// Long chats follow, each summarised and judged in one go so every
		// finished chat is saved even if the run stops partway.
		const ready = todo.filter((c) => transcripts.has(c.id));
		const isLong = (c: IndexedConversation) => (transcripts.get(c.id)?.approx_tokens ?? 0) > FULL_TRANSCRIPT_MAX_TOKENS;
		const short = ready.filter((c) => !isLong(c));
		let long = ready.filter(isLong);
		const needSummary = long.filter((c) => !this.store.summary(c.id, c.update_time, SUMMARY_PROMPT_VERSION));

		if (needSummary.length && !summariserAvailable()) {
			heldBack.push(...needSummary);
			long = long.filter((c) => !needSummary.includes(c));
			note(`${needSummary.length} long chat(s) need a summary but neither codex nor claude is on PATH; skipping them in step 3.`);
		} else if (needSummary.length) {
			const tokens = needSummary.reduce((sum, c) => sum + (transcripts.get(c.id)?.approx_tokens ?? 0), 0);
			// Codex adds ~19k tokens of its own instructions per call (measured 2026-09-27).
			const estimate = ((tokens + needSummary.length * 19_000) * PRICES["gpt-6-luna"].input) / 1e6;
			note(
				`${long.length} long chat(s) for step 3; ${needSummary.length} need a new summary (${summariserNames().join(", then ")}), ` +
					`~${Math.round(tokens / 1000)}k tokens, about ${formatUsd(estimate)} API-equivalent on your subscription.`,
			);
			if (tokens > CONFIRM_ABOVE_TOKENS && !opts.yes && !/^y(es)?$/i.test(await ask("Go ahead? [y/N] "))) {
				heldBack.push(...needSummary);
				long = long.filter((c) => !needSummary.includes(c));
				note("Skipping those in step 3; everything else will still be judged.");
			}
		}

		const judgeAll = async (label: string, items: IndexedConversation[]) => {
			if (!items.length) {
				note(`${label}: nothing to do.`);
				return;
			}
			const step = new Step(label, items.length);
			let done = 0;
			const failedBefore = failures.length;
			await runPool(items, CONCURRENCY, async (c) => {
				try {
					judgments.set(c.id, await this.judge(c, transcripts.get(c.id) as CachedTranscript));
				} catch (err) {
					fail(c, err);
				}
				step.update(++done, formatUsd(this.meter.total()));
			});
			const failed = failures.length - failedBefore;
			step.finish(`${label}: ${done - failed} judged${failed ? `, ${failed} failed` : ""}, ${formatUsd(this.meter.total())} so far`);
		};
		await judgeAll("[2/3] Judging short chats", short);
		await judgeAll("[3/3] Summarising and judging long chats", long);
		for (const line of this.meter.report()) note(line);
		return { judgments, failures, heldBack };
	}
}

// Runs `fn` over items with at most `concurrency` in flight. Small enough not
// to warrant a dependency.
export async function runPool<T>(items: T[], concurrency: number, fn: (item: T) => Promise<void>) {
	let next = 0;
	const worker = async () => {
		while (next < items.length) await fn(items[next++] as T);
	};
	await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, worker));
}
