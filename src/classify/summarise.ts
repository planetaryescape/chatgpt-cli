import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { findSecret } from "../auth/secrets.ts";
import { priceOf } from "./costs.ts";
import { anthropicText, openAIText } from "./model-api.ts";

// Bump when INSTRUCTIONS change so cached summaries are regenerated.
export const SUMMARY_PROMPT_VERSION = 9;

// The summary exists only to let Jev judge a chat too long for its window, so it
// keeps exactly the facts the questions in questions.ts depend on.
const INSTRUCTIONS = `You summarise a ChatGPT conversation so a separate classifier can decide whether the user should keep, archive, or delete it. You never see the classifier; write for it.

Write plain prose under 600 words covering:
- What the user was trying to do or find out.
- What the conversation produced: answers, decisions, plans, drafts, code, or documents. Name them specifically.
- Whether the user developed an idea for a piece or project they want to create, such as an article, essay, book, talk, sermon, devotional, Bible study, app, or product. Drafting or outlining a piece counts. If so, say "The user was brainstorming" and name its primary purpose and intended form. For products, distinguish the user's own new concept or substantial feature direction from routine implementation, debugging, tutorials, interview exercises, hypothetical examples, marketing copy for a settled product, and work on another organization's product; only the former is product brainstorming. Distinguish a piece primarily meant for religious teaching or reflection from broader writing that draws on faith as one influence or theme. Do not call the latter a religious piece just because it mentions faith, Christianity, Scripture, or a Christian writer. A faith question without a piece being developed is not brainstorming. Conversations often drift: life advice or a question can turn into shaping an article or an app idea.
- Whether it was quick help the user could get again just by asking again: general knowledge, curiosity, a how-to, or a practical problem, answered, even with follow-ups and mentions of their own situation. If so, say "This was quick help that could be re-asked". Name anything that could not be re-asked: decisions, personal records, plans, drafts, the user's own data or writing.
- Whether its practical value was tied to a particular date, event, deadline, trip, booking, purchase, or live situation. State the date or timing if known, whether that moment has passed, and whether anything of lasting value remains. Personal details used only to answer one-off logistics are context, not necessarily a lasting record. A claim, meaningful decision, financial or legal record, reusable artifact, original thinking, or work to resume remains valuable after an event.
- How it ended: resolved, abandoned, or left in progress, and what was still open.
- Any personal records in it (health, money, legal, housing, family, important decisions), stated factually.
- Whether most of it is substantive or filler.

Do not judge whether it is worth keeping. Do not add advice. Output only the summary.`;

type RunResult = { text: string; usd: number; tokens: number };
type Summariser = { name: string; binary: string; run: (input: string) => Promise<RunResult> };

export type Summary = { summary: string; model: string; usd: number; tokens: number };

async function exec(cmd: string[], input: string, cwd: string): Promise<{ code: number; out: string; err: string }> {
	const proc = Bun.spawn(cmd, { cwd, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
	proc.stdin.write(input);
	proc.stdin.end();
	const [out, err, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
	return { code, out, err };
}

// Subscription CLIs run from scratch paths so project instructions do not leak.
const SUMMARISERS: Summariser[] = [
	{
		// Codex's own instructions still cost ~19k
		// input tokens per call; the flags strip tools and user config.
		name: "gpt-6-luna",
		binary: "codex",
		async run(input) {
			const dir = mkdtempSync(join(tmpdir(), "chatgpt-cli-codex-"));
			try {
				const outFile = join(dir, "summary.txt");
				const noTools = ["apps", "browser_use", "computer_use", "image_generation", "plugins", "multi_agent", "view_image", "unified_exec", "shell_tool"];
				const { code, out, err } = await exec(
					[
						"codex", "exec", "-m", "gpt-6-luna", "-c", 'model_reasoning_effort="medium"', "--skip-git-repo-check", "--ephemeral", "--sandbox", "read-only",
						"--ignore-user-config", ...noTools.flatMap((f) => ["--disable", f]), "-C", dir, "-o", outFile, "--json",
						`${INSTRUCTIONS}\n\nThe conversation is in the <stdin> block.`,
					],
					input,
					dir,
				);
				if (code !== 0) throw new Error(`codex exited ${code}: ${err.trim().slice(-300)}`);
				// --json streams events; token usage arrives on turn.completed.
				type CodexUsage = { input_tokens?: number; cached_input_tokens?: number; output_tokens?: number; reasoning_output_tokens?: number };
				let usage: CodexUsage = {};
				for (const line of out.split("\n")) {
					if (!line.includes('"turn.completed"')) continue;
					try {
						usage = (JSON.parse(line) as { usage?: CodexUsage }).usage ?? usage;
					} catch {
						// A malformed event line only costs us the usage figure.
					}
				}
				const u = {
					inputTokens: usage.input_tokens ?? 0,
					cachedInputTokens: usage.cached_input_tokens ?? 0,
					// Reasoning tokens are billed as output.
					outputTokens: (usage.output_tokens ?? 0) + (usage.reasoning_output_tokens ?? 0),
				};
				return { text: readFileSync(outFile, "utf8"), usd: priceOf("gpt-6-luna", u), tokens: u.inputTokens + u.outputTokens };
			} finally {
				rmSync(dir, { recursive: true, force: true });
			}
		},
	},
	{
		// Fallback. Haiku is plenty for summarising. --effort low made it ignore
		// the system prompt (observed 2026-09-27); medium and high behave.
		name: "claude-haiku",
		binary: "claude",
		async run(input) {
			const { code, out, err } = await exec(
				[
					"claude", "-p", "--model", "haiku", "--output-format", "json", "--tools", "", "--no-session-persistence",
					// Without these, every call carries ~33k tokens of MCP and settings context.
					"--setting-sources", "", "--strict-mcp-config", "--effort", "medium", "--system-prompt", INSTRUCTIONS,
				],
				input,
				tmpdir(),
			);
			let parsed: {
				is_error?: boolean;
				result?: string;
				total_cost_usd?: number;
				usage?: { input_tokens?: number; cache_read_input_tokens?: number; cache_creation_input_tokens?: number; output_tokens?: number };
			};
			try {
				parsed = JSON.parse(out);
			} catch {
				throw new Error(`claude -p exited ${code}: ${(err || out).trim().slice(0, 300)}`);
			}
			if (parsed.is_error) throw new Error(`claude -p failed: ${(parsed.result ?? err).slice(0, 300)}`);
			const u = parsed.usage ?? {};
			const tokens = (u.input_tokens ?? 0) + (u.cache_read_input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0) + (u.output_tokens ?? 0);
			// claude -p reports its own API-equivalent cost.
			return { text: parsed.result ?? "", usd: parsed.total_cost_usd ?? 0, tokens };
		},
	},
];

const openAI: Summariser = { name: "gpt-6-luna (API)", binary: "", run: (input) => openAIText(INSTRUCTIONS, input) };
const anthropic: Summariser = { name: "claude-haiku (API)", binary: "", run: (input) => anthropicText(INSTRUCTIONS, input) };

const installed = (): Summariser[] => [
	findSecret("OPENAI_API_KEY") ? openAI : Bun.which("codex") ? SUMMARISERS[0] : null,
	findSecret("ANTHROPIC_API_KEY") ? anthropic : Bun.which("claude") ? SUMMARISERS[1] : null,
].filter((s): s is Summariser => Boolean(s));

export function summariserNames(): string[] {
	return installed().map((s) => s.name);
}

export function summariserAvailable(): boolean {
	return installed().length > 0;
}

// Tries each installed summariser in order; a failure falls through to the next.
export async function summarise(title: string, transcript: string): Promise<Summary> {
	const errors: string[] = [];
	for (const s of installed()) {
		try {
			const { text, usd, tokens } = await s.run(`Title: ${title}\n\n${transcript}`);
			const summary = text.trim();
			if (summary) return { summary, model: s.name, usd, tokens };
			errors.push(`${s.name}: empty summary`);
		} catch (err) {
			errors.push(`${s.name}: ${err instanceof Error ? err.message : String(err)}`);
		}
	}
	throw new Error(errors.length ? `No summariser succeeded. ${errors.join(" | ")}` : "Configure an OpenAI or Anthropic API key, or install codex or claude.");
}
