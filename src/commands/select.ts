import { type BrainstormKind, type Suggestion, topicOf, verdictOf } from "../classify/policy.ts";
import { displayTitle } from "../index/store.ts";
import { QUESTIONS_VERSION, TOPICS } from "../classify/questions.ts";
import type { ClassificationStore, Judgment } from "../index/classification-store.ts";
import type { ConversationIndex, Filter, IndexedConversation } from "../index/store.ts";
import { note } from "../progress.ts";

export type SelectOptions = {
	olderThan?: string;
	newerThan?: string;
	before?: string;
	after?: string;
	title?: string;
	archived?: boolean;
	all?: boolean;
	pinned?: boolean;
	limit?: string;
	suggest?: string;
	// Internal to bulk delete recommendations; apply before --limit.
	excludeUnsure?: boolean;
	topic?: string;
	// Commander gives `true` for a bare --brainstorm, or the kind when one is named.
	brainstorm?: string | boolean;
};

const UNIT_DAYS: Record<string, number> = { d: 1, w: 7, m: 30, y: 365 };

export function parseAge(value: string, now = Date.now()): Date {
	const match = /^(\d+)([dwmy])$/.exec(value);
	if (!match) throw new Error(`Invalid age "${value}". Use a number and unit, e.g. 30d, 12w, 6m, 2y.`);
	const days = Number(match[1]) * (UNIT_DAYS[match[2] as string] ?? 1);
	return new Date(now - days * 86_400_000);
}

function parseDate(value: string): Date {
	const date = new Date(value);
	if (Number.isNaN(date.getTime())) throw new Error(`Invalid date "${value}". Use YYYY-MM-DD.`);
	return date;
}

export function toFilter(opts: SelectOptions): Filter {
	// When both an age and a date bound are given, the stricter one wins.
	const earliest = (a?: Date, b?: Date) => (a && b ? new Date(Math.min(a.getTime(), b.getTime())) : (a ?? b));
	const latest = (a?: Date, b?: Date) => (a && b ? new Date(Math.max(a.getTime(), b.getTime())) : (a ?? b));
	return {
		archived: opts.all ? undefined : Boolean(opts.archived),
		includePinned: opts.pinned,
		updatedBefore: earliest(opts.olderThan ? parseAge(opts.olderThan) : undefined, opts.before ? parseDate(opts.before) : undefined),
		updatedAfter: latest(opts.newerThan ? parseAge(opts.newerThan) : undefined, opts.after ? parseDate(opts.after) : undefined),
		titleMatch: opts.title ? new RegExp(opts.title, "i") : undefined,
		// --limit is applied last, in applyJevFiltersAndLimit, so it counts matches
		// after the Jev filters rather than before them.
	};
}

export function hasFilter(opts: SelectOptions): boolean {
	return Boolean(opts.olderThan || opts.newerThan || opts.before || opts.after || opts.title || opts.limit || opts.suggest || opts.topic || opts.brainstorm);
}

async function readStdinIds(): Promise<string[]> {
	const text = await Bun.stdin.text();
	// Accept `list` output directly: the id is the first column.
	return text.split("\n").map((l) => l.trim().split(/\s+/)[0] ?? "").filter(Boolean);
}

export function requireSynced(index: ConversationIndex) {
	const syncedAt = index.syncedAt();
	if (!syncedAt) throw new Error("No local index yet. Run `chatgpt sync` first.");
	const hours = (Date.now() - syncedAt.getTime()) / 3_600_000;
	if (hours > 24) note(`note: index last synced ${Math.round(hours / 24)}d ago; run \`chatgpt sync\` to refresh.`);
}

// Resolves command targets from explicit ids (full or prefix), `-` for stdin, or filters.
export async function selectTargets(
	index: ConversationIndex,
	ids: string[],
	opts: SelectOptions,
	{ allowUnfiltered = false, store }: { allowUnfiltered?: boolean; store?: ClassificationStore } = {},
): Promise<IndexedConversation[]> {
	requireSynced(index);
	if (ids.length === 0) {
		if (!allowUnfiltered && !hasFilter(opts)) {
			throw new Error("Pass conversation ids, `-` to read ids from stdin, or a filter such as --older-than 1y or --title.");
		}
		return applyJevFiltersAndLimit(index.query(toFilter(opts)), opts, store);
	}
	const raw = ids.length === 1 && ids[0] === "-" ? await readStdinIds() : ids;
	const found: IndexedConversation[] = [];
	for (const id of raw) {
		const matches = index.get(id);
		if (matches.length === 0) throw new Error(`No conversation matching "${id}" in the index. Run \`chatgpt sync\`?`);
		if (matches.length > 1) throw new Error(`"${id}" matches ${matches.length} conversations; use a longer prefix.`);
		const target = matches[0] as IndexedConversation;
		if (!opts.all && Boolean(target.is_archived) !== Boolean(opts.archived)) {
			throw new Error(`Conversation "${id}" is ${target.is_archived ? "archived; pass --archived or --all to include it" : "active; omit --archived or pass --all to include it"}.`);
		}
		found.push(target);
	}
	return found;
}

export async function selectOne(index: ConversationIndex, id: string, opts: SelectOptions = {}): Promise<IndexedConversation> {
	const [target] = await selectTargets(index, [id], opts);
	if (!target) throw new Error(`No conversation matching "${id}".`);
	return target;
}

const SUGGESTIONS: readonly Suggestion[] = ["delete", "archive", "keep"];
export const BRAINSTORM_KINDS: readonly BrainstormKind[] = ["writing", "sermon", "product", "other"];

// --suggest and --topic narrow by Jev's current judgments; unjudged chats never match.
export function applyJevFiltersAndLimit(rows: IndexedConversation[], opts: SelectOptions, store?: ClassificationStore): IndexedConversation[] {
	const limit = opts.limit ? Number(opts.limit) : undefined;
	if (limit !== undefined && (!Number.isInteger(limit) || limit < 1)) throw new Error(`--limit must be a positive whole number.`);
	const take = (r: IndexedConversation[]) => (limit ? r.slice(0, limit) : r);
	if (!opts.suggest && !opts.topic && !opts.brainstorm) return take(rows);
	const kind = typeof opts.brainstorm === "string" ? opts.brainstorm : null;
	if (kind && !BRAINSTORM_KINDS.includes(kind as BrainstormKind)) throw new Error(`--brainstorm takes ${BRAINSTORM_KINDS.join(", ")}, or nothing for any.`);
	if (opts.suggest && !SUGGESTIONS.includes(opts.suggest as Suggestion)) {
		throw new Error(`--suggest must be one of ${SUGGESTIONS.join(", ")}.`);
	}
	if (opts.topic && !(opts.topic in TOPICS)) throw new Error(`--topic must be one of ${Object.keys(TOPICS).join(", ")}.`);
	if (!store) throw new Error("Judgment filters need the classification store.");
	const judgments = store.currentJudgments(QUESTIONS_VERSION);
	return take(rows.filter((r) => {
		const j = judgments.get(r.id);
		if (!j) return false;
		const v = verdictOf(j);
		if (opts.suggest && v.suggestion !== opts.suggest) return false;
		if (opts.excludeUnsure && v.unsure) return false;
		if (opts.brainstorm && (!v.brainstorm || (kind && v.brainstorm !== kind))) return false;
		return !opts.topic || topicOf(j) === opts.topic;
	}));
}

export function formatRow(c: IndexedConversation, judgment?: Judgment): string {
	const flags = `${c.is_archived ? "A" : " "}${c.pinned ? "P" : " "}${c.project_id ? "J" : " "}`;
	let jev = "";
	if (judgment) {
		const v = verdictOf(judgment);
		const idea = v.brainstorm ? `idea:${v.brainstorm}` : "";
		jev = `${`${v.suggestion}${v.unsure ? "?" : ""}`.padEnd(9)}${topicOf(judgment).padEnd(21)}${idea.padEnd(14)}`;
	}
	return `${c.id}  ${c.update_time.slice(0, 10)}  ${flags}  ${jev}${displayTitle(c)}`;
}
