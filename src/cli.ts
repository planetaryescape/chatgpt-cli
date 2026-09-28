#!/usr/bin/env bun
import { Command } from "commander";
import { ApiError, ChatGPTClient } from "./api/client.ts";
import { BATCH_MAX, type ConversationSummary, getConversation, getConversationsBatch, listConversations, renameConversation, searchConversations } from "./api/conversations.ts";
import { listProjects } from "./api/projects.ts";
import { getMemorySummary, listMemories } from "./api/memories.ts";
import { classifyMemories, memoryCounts } from "./classify/memories.ts";
import { Classifier } from "./classify/pipeline.ts";
import { DeepClassifier } from "./classify/deep-pipeline.ts";
import { classifyRemainingWithLuna } from "./classify/luna-pipeline.ts";
import { generateLocalTitles } from "./classify/titles.ts";
import { baseVerdictOf, verdictOf } from "./classify/policy.ts";
import { QUESTIONS_VERSION, TOPICS } from "./classify/questions.ts";
import { checkWithJev } from "./commands/check.ts";
import { exportConversation } from "./commands/export.ts";
import { printMemoryStats, printStats } from "./commands/stats.ts";
import { applyAction, type Action } from "./commands/mutate.ts";
import { applyProjectAdd, applyProjectRemove, resolveProject } from "./commands/projects.ts";
import { deleteSelectedMemories, formatClassifiedMemories, formatMemories, formatMemorySummary, selectClassifiedMemories, type MemoryFormat, writeData } from "./commands/memories.ts";
import { configPath, readConfig, readKeyFromStdin, setConfigKey, type Provider } from "./auth/config.ts";
import { review } from "./commands/review.ts";
import { applyJevFiltersAndLimit, formatRow, requireSynced, type SelectOptions, selectOne, selectTargets, toFilter } from "./commands/select.ts";
import { ClassificationStore } from "./index/classification-store.ts";
import { MemoryClassificationStore } from "./index/memory-classification-store.ts";
import { ConversationIndex, displayTitle } from "./index/store.ts";
import { reconcileMetadataChanges } from "./index/reconcile-metadata.ts";
import { reconcileFullSyncOmissions } from "./index/reconcile-full-sync.ts";
import { note, Step, timer } from "./progress.ts";
import { LocalEmbedder } from "./search/embeddings.ts";
import { SearchIndexer } from "./search/indexer.ts";
import { formatSearchResults, type SearchOutputFormat, type SearchResult } from "./search/output.ts";
import { searchLocal, type SearchMode } from "./search/query.ts";
import { SearchStore } from "./search/store.ts";

const client = new ChatGPTClient();
const index = new ConversationIndex();
const store = new ClassificationStore(index.db);
const memoryStore = new MemoryClassificationStore(index.db);
const classifier = new Classifier(client, store);
const deepClassifier = new DeepClassifier(client, store);
const searchStore = new SearchStore(index.db);
const embedder = new LocalEmbedder();
const searchIndexer = new SearchIndexer(client, index, store, searchStore, embedder);
const program = new Command("chatgpt").description("Manage your ChatGPT conversations from the terminal (uses your Dia login).");

function positiveLimit(raw: string): number {
	const limit = Number(raw);
	if (!Number.isSafeInteger(limit) || limit < 1) throw new Error("--limit must be a positive integer.");
	return limit;
}

program.command("configure [provider]")
	.description("Store Jev, OpenAI, or Anthropic API keys in the user config; omit provider to show status")
	.option("--remove", "remove the selected provider's stored key")
	.action(async (provider: string | undefined, opts: { remove?: boolean }) => {
		if (!provider) {
			if (opts.remove) throw new Error("Choose a provider to remove: jev, openai, or anthropic.");
			const config = readConfig();
			await writeData(`Config: ${configPath()}\n${(["jev", "openai", "anthropic"] as const)
				.map((name) => `${name}: ${config[name] ? "configured" : "not configured"}`).join("\n")}`);
			return;
		}
		if (!["jev", "openai", "anthropic"].includes(provider)) throw new Error("Provider must be jev, openai, or anthropic.");
		if (opts.remove) {
			setConfigKey(provider as Provider, null);
			note(`Removed stored ${provider} key.`);
			return;
		}
		if (process.stdin.isTTY) note(`Enter ${provider} API key (hidden; press Enter to save):`);
		const key = await readKeyFromStdin();
		if (!key) throw new Error("API key cannot be empty.");
		setConfigKey(provider as Provider, key);
		note(`Saved ${provider} key to ${configPath()}.`);
	});

function withFilters(cmd: Command, { pinnedFlag }: { pinnedFlag: boolean }): Command {
	cmd
		.option("--older-than <age>", "last updated more than <age> ago (30d, 12w, 6m, 2y)")
		.option("--newer-than <age>", "last updated within <age>")
		.option("--before <date>", "last updated before YYYY-MM-DD")
		.option("--after <date>", "last updated on or after YYYY-MM-DD")
		.option("--title <regex>", "title matches regex (case-insensitive)")
		.option("--archived", "archived conversations instead of active ones")
		.option("--all", "both active and archived")
		.option("--limit <n>", "at most n conversations (newest first)")
		.option("--suggest <action>", "only chats classified as delete, archive, or keep (needs `classify`)")
		.option("--topic <topic>", `only chats classified in this topic: ${Object.keys(TOPICS).join(", ")}`)
		.option("--brainstorm [kind]", "only chats where you were brainstorming; optionally writing, sermon, product, or other");
	if (pinnedFlag) cmd.option("--pinned", "include pinned conversations (skipped by default)");
	return cmd;
}

program
	.command("sync")
	.description("Update the local index: new and changed chats, plus archive state")
	.option("--full", "rebuild from scratch (also drops chats deleted in the browser)")
	.action(async (opts: { full?: boolean }) => {
		const watermark = index.activeWatermark();
		if (opts.full || !watermark) await fullSync();
		else await deltaSync(watermark);
	});

async function fullSync() {
	const elapsed = timer();
	const startedAt = new Date().toISOString();
	const byId = new Map<string, ConversationSummary>();
	for (const archived of [false, true]) {
		const step = new Step(`Listing ${archived ? "archived" : "active"} chats`);
		let n = 0;
		for await (const c of listConversations(client, archived)) {
			if (!byId.has(c.id)) byId.set(c.id, c);
			step.update(++n);
		}
		step.finish(`Listed ${n} ${archived ? "archived" : "active"} chat(s)`);
	}
	// A chat updated during the sync jumps to the top, possibly past the pages
	// already read. Re-read the top until reaching chats older than the start.
	for (const archived of [false, true]) {
		for await (const c of listConversations(client, archived)) {
			if (c.update_time < startedAt) break;
			byId.set(c.id, c);
		}
	}
	const omissions = await reconcileFullSyncOmissions(byId, index.query({ includePinned: true }), (id) => getConversation(client, id));
	if (omissions.recovered) note(`Recovered ${omissions.recovered} chat(s) omitted from the conversation lists after individual checks.`);
	const all = [...byId.values()];
	const before = index.query({ includePinned: true }).length;
	index.replaceAll(all);
	const reconciled = await reconcileMetadataChanges(client, index, all.map((c) => c.id));
	if (reconciled.failures.length) process.exitCode = 1;
	const archived = all.filter((c) => c.is_archived).length;
	note(
		`Full sync: ${all.length} chats (${all.length - archived} active, ${archived} archived), ` +
			`${all.length - before >= 0 ? "+" : ""}${all.length - before} vs before, in ${elapsed()}.`,
	);
}

async function deltaSync(watermark: string) {
	const elapsed = timer();
	const changed = new Map<string, ConversationSummary>();
	const known = new Set(index.query({ includePinned: true }).map((c) => c.id));
	const step = new Step("Checking for new and updated chats");
	for await (const c of listConversations(client, false)) {
		if (c.update_time <= watermark) break;
		changed.set(c.id, c);
		step.update(changed.size);
	}
	const added = [...changed.keys()].filter((id) => !known.has(id)).length;
	step.finish(`${added} new, ${changed.size - added} updated`);

	const archivedStep = new Step("Reading archived chats");
	const archived = new Map<string, ConversationSummary>();
	for await (const c of listConversations(client, true)) {
		archived.set(c.id, c);
		archivedStep.update(archived.size);
	}
	archivedStep.finish(`${archived.size} archived`);

	// Chats that left the archived list were unarchived or deleted. The list has
	// once come back short right after archive changes, so confirm each one.
	const wasArchived = new Set(index.archivedIds());
	const dropped = [...wasArchived].filter((id) => !archived.has(id));
	const newlyArchived = [...archived.keys()].filter((id) => !wasArchived.has(id)).length;
	index.applyDelta([...changed.values()], [...archived.values()]);
	let unarchived = 0;
	let deleted = 0;
	for (let i = 0; i < dropped.length; i += BATCH_MAX) {
		const ids = dropped.slice(i, i + BATCH_MAX);
		const found = new Map((await getConversationsBatch(client, ids)).map((c) => [c.conversation_id, c]));
		for (const id of ids) {
			const convo = found.get(id);
			if (!convo) {
				index.remove(id);
				deleted++;
			} else if (!convo.is_archived) {
				index.setArchived(id, false);
				unarchived++;
			}
		}
	}
	const reconciled = await reconcileMetadataChanges(client, index, index.query({ includePinned: true }).map((c) => c.id));
	if (reconciled.failures.length) process.exitCode = 1;
	const parts = [`${added} new`, `${changed.size - added} updated`, `${newlyArchived} newly archived`, `${unarchived} unarchived`, `${deleted} deleted`];
	note(`Sync done in ${elapsed()}: ${parts.join(", ")}. \`sync --full\` also drops chats deleted in the browser.`);
}

withFilters(program.command("list").description("List conversations from the local index"), { pinnedFlag: false })
	.option("--json", "output JSON")
	.option("--format <format>", "output format: ids (default: text)")
	.option("--count", "print only the number of matches")
	.action(async (opts: SelectOptions & { json?: boolean; format?: string; count?: boolean }) => {
		if (opts.format && opts.format !== "ids") throw new Error("--format must be ids.");
		if (opts.json && opts.format) throw new Error("Choose only one of --json or --format.");
		if (opts.count && opts.format) throw new Error("Choose only one of --count or --format.");
		requireSynced(index);
		const rows = applyJevFiltersAndLimit(index.query({ ...toFilter(opts), includePinned: true }), opts, store);
		const judgments = store.currentJudgments(QUESTIONS_VERSION);
		if (opts.count) console.log(rows.length);
		else if (opts.format === "ids") for (const row of rows) console.log(row.id);
		else if (opts.json) {
			const withJev = rows.map((r) => {
				const j = judgments.get(r.id);
				return j ? { ...r, display_title: displayTitle(r), topic: j.topic, jev: { ...verdictOf(j), answers: JSON.parse(j.answers) } } : { ...r, display_title: displayTitle(r), topic: null };
			});
			// Bun can exit before a large JSON write drains to a pipe.
			await new Promise<void>((resolve, reject) => {
				process.stdout.write(`${JSON.stringify(withJev, null, 2)}\n`, (error) => error ? reject(error) : resolve());
			});
		} else for (const r of rows) console.log(formatRow(r, judgments.get(r.id)));
	});

withFilters(program.command("stats").description("Chat suggestions, brainstorms and topics, plus saved-memory suggestions"), { pinnedFlag: false }).action(
	async (opts: SelectOptions) => {
		requireSynced(index);
		printStats(applyJevFiltersAndLimit(index.query({ ...toFilter(opts), includePinned: true }), opts, store), store);
		try {
			printMemoryStats(memoryCounts(await listMemories(client), memoryStore));
		} catch (err) {
			console.error(`Saved-memory stats unavailable: ${err instanceof Error ? err.message : String(err)}`);
			process.exitCode = 1;
		}
	},
);

const memoryCommand = program.command("memory").description("List and delete saved memories, or read the memory summary");

memoryCommand.command("list")
	.description("List saved memories from ChatGPT")
	.option("--search <text>", "only memories whose content contains text")
	.option("--limit <n>", "at most n saved memories")
	.option("--format <format>", "output format: json, csv, table, or ids (default: table)", "table")
	.action(async (opts: { search?: string; limit?: string; format: string }) => {
		if (!["json", "csv", "table", "ids"].includes(opts.format)) throw new Error("--format must be json, csv, table, or ids.");
		const memories = await listMemories(client);
		const search = opts.search?.toLocaleLowerCase();
		const selected = (search ? memories.filter((m) => m.content.toLocaleLowerCase().includes(search)) : memories)
			.slice(0, opts.limit ? positiveLimit(opts.limit) : undefined);
		await writeData(formatMemories(selected, opts.format as MemoryFormat));
	});

memoryCommand.command("classify")
	.description("Classify saved memories for keep, delete, or review with Jev and Luna; never deletes")
	.option("--suggest <action>", "show only keep, delete, or review")
	.option("--limit <n>", "at most n results after filtering")
	.option("--format <format>", "output format: json, csv, table, or ids (default: table)", "table")
	.option("--redo", "reclassify all saved memories")
	.action(async (opts: { suggest?: string; limit?: string; format: string; redo?: boolean }) => {
		if (!["json", "csv", "table", "ids"].includes(opts.format)) throw new Error("--format must be json, csv, table, or ids.");
		if (opts.suggest && !["keep", "delete", "review"].includes(opts.suggest)) throw new Error("--suggest must be keep, delete, or review.");
		const result = await classifyMemories(await listMemories(client), memoryStore, { redo: opts.redo });
		const rows = selectClassifiedMemories(result.rows, opts.suggest as "keep" | "delete" | "review" | undefined)
			.slice(0, opts.limit ? positiveLimit(opts.limit) : undefined);
		await writeData(formatClassifiedMemories(rows, opts.format as MemoryFormat));
		for (const failure of result.failures) console.error(`failed: ${failure}`);
		if (result.failures.length) process.exitCode = 1;
	});

memoryCommand.command("summary")
	.description("Read ChatGPT's generated memory summary")
	.option("--format <format>", "output format: json or table (default: table)", "table")
	.action(async (opts: { format: string }) => {
		if (opts.format !== "json" && opts.format !== "table") throw new Error("--format must be json or table.");
		await writeData(formatMemorySummary(await getMemorySummary(client), opts.format));
	});

memoryCommand.command("delete <ids...>")
	.description("Delete saved memories by id/prefix or `-` for ids on stdin; does not delete source chats")
	.option("-n, --dry-run", "preview without deleting memories")
	.option("-y, --yes", "skip the confirmation prompt")
	.action((ids: string[], opts: { dryRun?: boolean; yes?: boolean }) => deleteSelectedMemories(client, ids, opts));

program
	.command("export <link>")
	.alias("show")
	.description("Export a conversation as markdown: chat link, id, or id prefix")
	.option("-o, --output [file]", "write to a file (default name: from the title)")
	.option("-c, --copy", "copy to the clipboard")
	.option("--archived", "allow an archived conversation")
	.option("--all", "allow either active or archived")
	.action((link: string, opts: { output?: string | boolean; copy?: boolean } & SelectOptions) => exportConversation(client, index, link, opts));

program.command("search-index")
	.description("Build or refresh the local text and semantic search index")
	.option("--archived", "index archived conversations instead of active ones")
	.option("--all", "index both active and archived conversations")
	.action(async (opts: SelectOptions) => {
		requireSynced(index);
		const archived = opts.all ? null : Boolean(opts.archived);
		const elapsed = timer();
		const result = await searchIndexer.build(archived);
		const coverage = searchStore.coverage(embedder.version, archived);
		for (const failure of result.failures) console.error(`failed: ${failure}`);
		note(`Search index in ${elapsed()}: ${coverage.indexed}/${coverage.chats} chats, ${coverage.chunks} text chunks, ${coverage.embedded} embeddings.`);
		if (result.failures.length) process.exitCode = 1;
	});

program.command("search <query>")
	.description("Search the local transcript index; use --semantic or --hybrid for meaning-based matches")
	.option("--semantic", "rank by local embedding similarity")
	.option("--hybrid", "combine full-text and semantic ranking")
	.option("--remote", "use ChatGPT's server-side search instead")
	.option("--archived", "search archived conversations instead of active ones")
	.option("--all", "search both active and archived conversations")
	.option("--format <format>", "output format: json, csv, table, or ids (default: two-line text)")
	.option("--limit <n>", "maximum conversations", "20")
	.action(async (query: string, opts: SelectOptions & { limit: string; semantic?: boolean; hybrid?: boolean; remote?: boolean; format?: string }) => {
		const limit = Number(opts.limit);
		if (!Number.isSafeInteger(limit) || limit < 1) throw new Error("--limit must be a positive integer.");
		if (Number(opts.semantic) + Number(opts.hybrid) + Number(opts.remote) > 1) throw new Error("Choose only one of --semantic, --hybrid, or --remote.");
		if (opts.remote && limit > 40) throw new Error("--remote supports --limit up to 40 (ChatGPT's search API limit).");
		const requestedFormat = opts.format;
		if (requestedFormat !== undefined && requestedFormat !== "json" && requestedFormat !== "csv" && requestedFormat !== "table" && requestedFormat !== "ids") {
			throw new Error("--format must be json, csv, table, or ids.");
		}
		const format: SearchOutputFormat | undefined = requestedFormat;
		const archived = opts.all ? undefined : Boolean(opts.archived);
		let results: SearchResult[];
		let localResultCount: number | undefined;
		if (opts.remote) {
			const hits = await searchConversations(client, query, opts.all ? limit : Math.min(40, limit * 5));
			const seen = new Set<string>();
			results = [];
			for (const h of hits) {
				if (archived !== undefined && Boolean(h.payload.is_archived) !== archived) continue;
				const id = h.payload.conversation_id;
				if (seen.has(id)) continue;
				seen.add(id);
				results.push({
					id, title: h.title, updated: new Date(h.update_time * 1000).toISOString(),
					archived: h.payload.is_archived, score: null,
					snippet: h.snippet.replace(/\s+/g, " ").slice(0, 160),
				});
				if (results.length === limit) break;
			}
		} else {
			requireSynced(index);
			const refreshed = searchIndexer.refreshCached(archived ?? null);
			const coverage = searchStore.coverage(embedder.version, archived ?? null);
			const indexCommand = `chatgpt search-index${opts.all ? " --all" : opts.archived ? " --archived" : ""}`;
			if (!coverage.indexed) throw new Error(`No local transcripts indexed. Run \`${indexCommand}\` first.`);
			if (refreshed.missing.length) note(`${refreshed.missing.length} chat(s) lack a cached transcript; run \`${indexCommand}\` to include them.`);
			const mode: SearchMode = opts.hybrid ? "hybrid" : opts.semantic ? "semantic" : "lexical";
			if (mode !== "lexical" && !coverage.embedded) throw new Error(`No local embeddings indexed. Run \`${indexCommand}\` first.`);
			if (mode !== "lexical" && coverage.embedded < coverage.chunks) note(`${coverage.chunks - coverage.embedded} chunk(s) lack current embeddings; run \`${indexCommand}\` to include them.`);
			results = await searchLocal(searchStore, embedder, query, mode, limit, archived ?? null);
			localResultCount = results.length;
		}
		results = results.map((r) => {
			const local = index.get(r.id)[0];
			return local ? { ...r, title: displayTitle(local) } : r;
		});
		const output = formatSearchResults(results, format);
		if (output) console.log(output);
		if (localResultCount !== undefined) note(`${localResultCount} conversation(s) found locally.`);
	});

const projectCommand = program.command("project").description("List projects and move chats into or out of them");

projectCommand.command("list")
	.description("List projects available to your account")
	.option("--json", "output JSON")
	.option("--limit <n>", "at most n projects")
	.action(async (opts: { json?: boolean; limit?: string }) => {
		const projects = (await listProjects(client)).slice(0, opts.limit ? positiveLimit(opts.limit) : undefined);
		if (opts.json) console.log(JSON.stringify(projects, null, 2));
		else for (const project of projects) console.log(`${project.id}  ${project.name}${project.canWrite ? "" : " (read-only)"}`);
	});

projectCommand.command("add <project> <ids...>")
	.description("Move chats into an existing project by name or id; use `-` for ids on stdin")
	.option("-n, --dry-run", "preview without moving chats")
	.option("-y, --yes", "skip the confirmation prompt")
	.option("--archived", "select archived conversations instead of active ones")
	.option("--all", "select both active and archived conversations")
	.action(async (reference: string, ids: string[], opts: SelectOptions & { dryRun?: boolean; yes?: boolean }) => {
		const targets = await selectTargets(index, ids, opts);
		const project = resolveProject(await listProjects(client), reference);
		await applyProjectAdd(client, index, project, targets, opts);
	});

projectCommand.command("remove <project> <ids...>")
	.description("Remove chats from an existing project; use `-` for ids on stdin")
	.option("-n, --dry-run", "preview without removing chats")
	.option("-y, --yes", "skip the confirmation prompt")
	.option("--archived", "select archived conversations instead of active ones")
	.option("--all", "select both active and archived conversations")
	.action(async (reference: string, ids: string[], opts: SelectOptions & { dryRun?: boolean; yes?: boolean }) => {
		const targets = await selectTargets(index, ids, opts);
		const project = resolveProject(await listProjects(client), reference);
		await applyProjectRemove(client, index, project, targets, opts);
	});

for (const action of ["archive", "unarchive", "delete"] as const satisfies readonly Action[]) {
	withFilters(
		program
			.command(`${action} [ids...]`)
			.description(`${action[0]?.toUpperCase()}${action.slice(1)} conversations by id/prefix, \`-\` for ids on stdin, or by filter`),
		{ pinnedFlag: true },
	)
		.option("-n, --dry-run", "show what would change")
		.option("-y, --yes", "skip the confirmation prompt")
		.option("--check", "ask Jev to read each chat and only act on those it agrees with (archive/delete)")
		.action(async (ids: string[], opts: SelectOptions & { dryRun?: boolean; yes?: boolean; check?: boolean }) => {
			// Unarchive works on archived chats unless the caller asked otherwise.
			const selection = action === "unarchive" && !opts.all ? { ...opts, archived: true } : opts;
			const applyingSuggestions = action !== "unarchive" && opts.suggest === action;
			let targets = await selectTargets(index, ids, { ...selection, excludeUnsure: applyingSuggestions }, { store });
			// Explicit ids bypass filters, so keep the Jev check as a second guard.
			if (opts.check || applyingSuggestions) {
				if (action === "unarchive") throw new Error("--check applies to archive and delete only.");
				targets = await checkWithJev(classifier, action, targets, opts);
			}
			await applyAction(client, index, action, targets, opts);
		});
}

program
	.command("rename <id> <title>")
	.description("Rename a conversation in ChatGPT (changes its order in the app)")
	.option("--archived", "allow an archived conversation")
	.option("--all", "allow either active or archived")
	.action(async (id: string, title: string, opts: SelectOptions) => {
		const target = await selectOne(index, id, opts);
		try {
			await renameConversation(client, target.id, title);
		} catch (err) {
			// Observed 2026-09-27: legacy (pre-2025) chats return 500 yet the sidebar
			// title does change, and no endpoint besides the full list reads it back.
			if (err instanceof ApiError && err.status === 500) {
				throw new Error("ChatGPT returned a server error. On older chats the rename usually applies anyway; run `chatgpt sync` and `chatgpt list --title` to check.");
			}
			throw err;
		}
		index.rename(target.id, title);
		note(`renamed "${target.title}" → "${title}"`);
	});

program.command("title <id> <title>")
	.description("Set a local display title without changing ChatGPT")
	.option("--archived", "allow an archived conversation")
	.option("--all", "allow either active or archived")
	.action(async (id: string, title: string, opts: SelectOptions) => {
		const target = await selectOne(index, id, opts);
		index.setLocalTitle(target, title);
		note(`Local title saved for ${target.id}: ${title.trim()}`);
	});

withFilters(program.command("titles [ids...]").description("Generate local display titles and topic themes with gpt-6-luna"), { pinnedFlag: true })
	.option("--redo", "regenerate Luna titles; manual titles are preserved")
	.action(async (ids: string[], opts: SelectOptions & { redo?: boolean }) => {
		const targets = await selectTargets(index, ids, opts, { allowUnfiltered: true, store });
		const failures = await generateLocalTitles(targets, index, store, opts);
		for (const failure of failures) console.error(`failed: ${failure}`);
		if (failures.length) process.exitCode = 1;
	});

withFilters(program.command("classify [ids...]").description("Classify with Jev and Luna, then generate missing local titles and topic themes"), {
	pinnedFlag: true,
})
	.option("--redo", "re-judge every matching chat, not just new or changed ones (reuses cached transcripts and summaries)")
	.option("-y, --yes", "don't ask before summarising a large batch")
	.action(async (ids: string[], opts: SelectOptions & { redo?: boolean; yes?: boolean }) => {
		const elapsed = timer();
		const targets = await selectTargets(index, ids, opts, { allowUnfiltered: true, store });
		const { judgments, failures, heldBack } = await classifier.classify(targets, { force: opts.redo, yes: opts.yes });
		const unsureTargets = targets.filter((c) => {
			const j = judgments.get(c.id);
			return j && baseVerdictOf(j).unsure;
		});
		const deep = unsureTargets.length ? await deepClassifier.classify(unsureTargets, { force: opts.redo }) : null;
		if (deep) for (const [id, j] of deep.judgments) judgments.set(id, j);
		const lunaFailures = await classifyRemainingWithLuna(targets, judgments, store, { force: opts.redo });
		const titleFailures = await generateLocalTitles(targets, index, store);
		const counts = { delete: 0, archive: 0, keep: 0, unsure: 0 };
		for (const j of judgments.values()) {
			const v = verdictOf(j);
			counts[v.suggestion]++;
			if (v.unsure) counts.unsure++;
		}
		for (const f of failures) console.error(`failed: ${f}`);
		for (const f of deep?.failures ?? []) console.error(`failed: ${f}`);
		for (const f of lunaFailures) console.error(`failed: ${f}`);
		for (const f of titleFailures) console.error(`failed: ${f}`);
		note(
			`Done in ${elapsed()}. ${judgments.size} of ${targets.length} judged: delete ${counts.delete}, archive ${counts.archive}, keep ${counts.keep} (${counts.unsure} still unsure).` +
				(heldBack.length ? ` ${heldBack.length} long chat(s) skipped for lack of a summary.` : "") +
				(deep?.heldBack ? ` ${deep.heldBack} deep classification(s) held back.` : ""),
		);
		note("Next: `chatgpt list --suggest delete`, then `chatgpt review --suggest delete`.");
		if (failures.length || deep?.failures.length || lunaFailures.length || titleFailures.length) process.exitCode = 1;
	});

withFilters(program.command("review [ids...]").description("Triage conversations one by one; changes apply after a final confirmation"), {
	pinnedFlag: true,
})
	.option("--oldest-first", "start from the oldest match")
	.action(async (ids: string[], opts: SelectOptions & { oldestFirst?: boolean }) => {
		const targets = await selectTargets(index, ids, opts, { allowUnfiltered: true, store });
		await review(client, index, store, opts.oldestFirst ? targets.reverse() : targets);
	});

program
	.command("tui")
	.description("Browse, filter and triage conversations in a terminal UI")
	.action(async () => {
		requireSynced(index);
		// Loaded lazily so other commands don't pay for OpenTUI's native library.
		const { runTui } = await import("./tui/main.tsx");
		await runTui(client, index, store);
	});

await program.parseAsync().catch((err: unknown) => {
	console.error(`error: ${err instanceof Error ? err.message : String(err)}`);
	process.exit(1);
});
