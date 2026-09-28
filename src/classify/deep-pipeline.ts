import { TypeSafeClient } from "@typesafe-ai/sdk";
import type { ChatGPTClient } from "../api/client.ts";
import { BATCH_MAX, getConversationsBatch } from "../api/conversations.ts";
import { requireSecret } from "../auth/secrets.ts";
import type { CachedTranscript, ClassificationStore, Judgment } from "../index/classification-store.ts";
import type { IndexedConversation } from "../index/store.ts";
import { note, Step } from "../progress.ts";
import { RENDER_VERSION, toCachedTranscript } from "../render/transcript.ts";
import { CostMeter, formatUsd, priceOf } from "./costs.ts";
import { DEEP_QUESTIONS, DEEP_QUESTIONS_VERSION } from "./deep-questions.ts";
import { FULL_TRANSCRIPT_MAX_TOKENS, runPool } from "./pipeline.ts";
import { baseVerdictOf, verdictOf } from "./policy.ts";
import { QUESTIONS_VERSION } from "./questions.ts";
import { SUMMARY_PROMPT_VERSION, summarise, summariserAvailable } from "./summarise.ts";

const CONCURRENCY = 4;
const BATCH_GAP_MS = 500;

export type DeepClassifyResult = { judgments: Map<string, Judgment>; failures: string[]; heldBack: number; cached: number };

export class DeepClassifier {
	private jev: TypeSafeClient | undefined;
	private meter = new CostMeter();

	constructor(private readonly chatgpt: ChatGPTClient, private readonly store: ClassificationStore) {}

	private jevClient(): TypeSafeClient {
		this.jev ??= new TypeSafeClient({ apiKey: requireSecret("TYPESAFE_API_KEY") });
		return this.jev;
	}

	private async judge(c: IndexedConversation, t: CachedTranscript): Promise<Judgment> {
		let content = t.markdown;
		let contentKind = "full transcript";
		if (t.approx_tokens > FULL_TRANSCRIPT_MAX_TOKENS) {
			let cached = this.store.summary(c.id, c.update_time, SUMMARY_PROMPT_VERSION);
			if (!cached) {
				if (!summariserAvailable()) throw new Error("Long chat needs a summary, but neither codex nor claude is on PATH.");
				const { summary, model, usd, tokens } = await summarise(c.title, t.markdown);
				this.meter.add(`summaries (${model})`, "subscription", usd, tokens);
				cached = { id: c.id, update_time: c.update_time, prompt_version: SUMMARY_PROMPT_VERSION, summary, model };
				this.store.saveSummary(cached);
			}
			content = cached.summary;
			contentKind = `summary of a long conversation (${t.turns} turns), written by another model`;
		}
		const result = await this.jevClient().systemOne({
			state: {
				conversation: {
					title: c.title,
					created: c.create_time.slice(0, 10),
					last_updated: c.update_time.slice(0, 10),
					turns: t.turns,
					in_a_project: Boolean(c.project_id),
					pinned: Boolean(c.pinned),
				},
				content_kind: contentKind,
				content,
			},
			questions: DEEP_QUESTIONS,
		});
		this.meter.add("Jev", "api", priceOf("jev", { inputTokens: result.usage.input_tokens, cachedInputTokens: 0, outputTokens: 0 }), result.usage.input_tokens);
		this.store.saveDeepJudgment({
			id: c.id,
			update_time: c.update_time,
			questions_version: QUESTIONS_VERSION,
			version: DEEP_QUESTIONS_VERSION,
			answers: JSON.stringify(result.answers),
			classified_at: new Date().toISOString(),
		});
		return this.store.judgment(c.id, c.update_time, QUESTIONS_VERSION) as Judgment;
	}

	async classify(targets: IndexedConversation[], opts: { force?: boolean } = {}): Promise<DeepClassifyResult> {
		this.meter = new CostMeter();
		const judgments = new Map<string, Judgment>();
		const failures: string[] = [];
		const todo: IndexedConversation[] = [];
		for (const c of targets) {
			const base = this.store.judgment(c.id, c.update_time, QUESTIONS_VERSION);
			if (!base || !baseVerdictOf(base).unsure) continue;
			const cached = opts.force ? null : this.store.deepJudgment(c.id, c.update_time, QUESTIONS_VERSION, DEEP_QUESTIONS_VERSION);
			if (cached) judgments.set(c.id, base);
			else todo.push(c);
		}
		const cached = judgments.size;
		note(`${targets.length} unsure chat(s): ${cached} already deep-classified, ${todo.length} to judge.`);
		if (!todo.length) return { judgments, failures, heldBack: 0, cached };

		const transcripts = new Map<string, CachedTranscript>();
		const missing = todo.filter((c) => {
			const t = this.store.transcript(c.id, c.update_time, RENDER_VERSION);
			if (t) transcripts.set(c.id, t);
			return !t;
		});
		if (missing.length) {
			const step = new Step("Downloading missing transcripts", missing.length);
			for (let i = 0; i < missing.length; i += BATCH_MAX) {
				const batch = missing.slice(i, i + BATCH_MAX);
				try {
					const found = new Map((await getConversationsBatch(this.chatgpt, batch.map((c) => c.id))).map((c) => [c.conversation_id, c]));
					for (const c of batch) {
						const convo = found.get(c.id);
						if (!convo) {
							failures.push(`${c.id} ${c.title}: not returned by ChatGPT; run sync.`);
							continue;
						}
						const t = toCachedTranscript(c.id, c.update_time, convo);
						this.store.saveTranscript(t);
						transcripts.set(c.id, t);
					}
				} catch (err) {
					for (const c of batch) failures.push(`${c.id} ${c.title}: ${err instanceof Error ? err.message : String(err)}`);
				}
				step.update(Math.min(i + BATCH_MAX, missing.length));
				if (i + BATCH_MAX < missing.length) await Bun.sleep(BATCH_GAP_MS);
			}
			step.finish(`Found transcripts for ${transcripts.size} chat(s)`);
		}
		const ready = todo.filter((c) => transcripts.has(c.id));
		const heldBack = todo.length - ready.length;
		const step = new Step("Deep-classifying", ready.length);
		let done = 0;
		await runPool(ready, CONCURRENCY, async (c) => {
			try {
				const judgment = await this.judge(c, transcripts.get(c.id) as CachedTranscript);
				judgments.set(c.id, judgment);
			} catch (err) {
				failures.push(`${c.id} ${c.title}: ${err instanceof Error ? err.message : String(err)}`);
			}
			step.update(++done, formatUsd(this.meter.total()));
		});
		step.finish(`Deep-classified ${judgments.size - cached} chat(s)${failures.length ? `, ${failures.length} failed` : ""}`);
		for (const line of this.meter.report()) note(line);
		return { judgments, failures, heldBack, cached };
	}
}
