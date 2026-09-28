import type { Database } from "bun:sqlite";

export type CachedTranscript = {
	id: string;
	update_time: string;
	render_version: number;
	markdown: string;
	turns: number;
	approx_tokens: number;
};
export type CachedSummary = { id: string; update_time: string; prompt_version: number; summary: string; model: string };
export type Judgment = {
	id: string;
	update_time: string;
	version: string;
	content_kind: "full" | "summary";
	// Jev's raw answers as JSON; suggestions are derived from them by policy.ts
	// on read, so tuning thresholds never needs a re-run.
	answers: string;
	classified_at: string;
	topic?: string;
	deep_answers?: string | null;
	deep_version?: string | null;
	luna_suggestion?: "delete" | "archive" | "keep" | null;
	luna_brainstorm?: "writing" | "sermon" | "product" | "other" | null;
	luna_reason?: string | null;
	luna_version?: number | null;
};

export type DeepJudgment = {
	id: string;
	update_time: string;
	questions_version: string;
	version: string;
	answers: string;
	classified_at: string;
};

// Every cached row records the conversation's update_time at the moment it was
// produced, so a chat that changed since is treated as stale and redone.
export class ClassificationStore {
	constructor(private readonly db: Database) {
		// Transcripts are a pure cache; an older schema is dropped, not migrated.
		const columns = db.query("select name from pragma_table_info('transcripts')").all() as { name: string }[];
		if (columns.length && !columns.some((c) => c.name === "render_version")) db.run("drop table transcripts");
		db.run(`create table if not exists transcripts (
			id text primary key, update_time text not null, render_version integer not null,
			markdown text not null, turns integer not null, approx_tokens integer not null
		)`);
		// Summaries written before prompt versioning can't be trusted to mention
		// brainstorming; drop them rather than migrate (they regenerate on demand).
		const summaryColumns = db.query("select name from pragma_table_info('summaries')").all() as { name: string }[];
		if (summaryColumns.length && !summaryColumns.some((c) => c.name === "prompt_version")) db.run("drop table summaries");
		db.run(`create table if not exists summaries (
			id text primary key, update_time text not null, prompt_version integer not null,
			summary text not null, model text not null
		)`);
		db.run(`create table if not exists judgments (
			id text primary key, update_time text not null, version text not null,
			content_kind text not null, answers text not null, classified_at text not null,
			topic text generated always as (json_extract(answers, '$.topic.choice')) virtual
		)`);
		const judgmentColumns = db.query("select name from pragma_table_xinfo('judgments')").all() as { name: string }[];
		if (!judgmentColumns.some((c) => c.name === "topic")) {
			db.run("alter table judgments add column topic text generated always as (json_extract(answers, '$.topic.choice')) virtual");
		}
		db.run(`create table if not exists deep_judgments (
			id text primary key, update_time text not null, questions_version text not null,
			version text not null, answers text not null, classified_at text not null
		)`);
		db.run(`create table if not exists luna_judgments (
			id text primary key, update_time text not null, questions_version text not null,
			deep_version text not null, version integer not null, suggestion text not null,
			brainstorm text, reason text not null, classified_at text not null
		)`);
	}

	transcript(id: string, updateTime: string, renderVersion: number): CachedTranscript | null {
		return this.db
			.query("select * from transcripts where id = ? and update_time = ? and render_version = ?")
			.get(id, updateTime, renderVersion) as CachedTranscript | null;
	}

	saveTranscript(t: CachedTranscript) {
		this.db
			.query("insert or replace into transcripts values (?, ?, ?, ?, ?, ?)")
			.run(t.id, t.update_time, t.render_version, t.markdown, t.turns, t.approx_tokens);
	}

	summary(id: string, updateTime: string, promptVersion: number): CachedSummary | null {
		return this.db
			.query("select * from summaries where id = ? and update_time = ? and prompt_version = ?")
			.get(id, updateTime, promptVersion) as CachedSummary | null;
	}

	saveSummary(s: CachedSummary) {
		this.db.query("insert or replace into summaries values (?, ?, ?, ?, ?)").run(s.id, s.update_time, s.prompt_version, s.summary, s.model);
	}

	judgment(id: string, updateTime: string, version: string): Judgment | null {
		return this.db
			.query(`select j.*, d.answers as deep_answers, d.version as deep_version,
				l.suggestion as luna_suggestion, l.brainstorm as luna_brainstorm, l.reason as luna_reason, l.version as luna_version
				from judgments j left join deep_judgments d
				on d.id = j.id and d.update_time = j.update_time and d.questions_version = j.version
				left join luna_judgments l on l.id = j.id and l.update_time = j.update_time and l.questions_version = j.version and l.deep_version = coalesce(d.version, '')
				where j.id = ? and j.update_time = ? and j.version = ?`)
			.get(id, updateTime, version) as Judgment | null;
	}

	deepJudgment(id: string, updateTime: string, questionsVersion: string, version: string): DeepJudgment | null {
		return this.db
			.query("select * from deep_judgments where id = ? and update_time = ? and questions_version = ? and version = ?")
			.get(id, updateTime, questionsVersion, version) as DeepJudgment | null;
	}

	saveDeepJudgment(j: DeepJudgment) {
		this.db.transaction(() => {
			this.db.query("delete from luna_judgments where id = ?").run(j.id);
			this.db.query("insert or replace into deep_judgments values (?, ?, ?, ?, ?, ?)")
				.run(j.id, j.update_time, j.questions_version, j.version, j.answers, j.classified_at);
		})();
	}

	lunaJudgment(id: string, updateTime: string, questionsVersion: string, deepVersion: string, version: number): boolean {
		return Boolean(this.db.query("select 1 from luna_judgments where id = ? and update_time = ? and questions_version = ? and deep_version = ? and version = ?")
			.get(id, updateTime, questionsVersion, deepVersion, version));
	}

	saveLunaJudgment(j: { id: string; update_time: string; questions_version: string; deep_version: string; version: number; suggestion: string; brainstorm: string | null; reason: string }) {
		this.db.query("insert or replace into luna_judgments values (?, ?, ?, ?, ?, ?, ?, ?, ?)")
			.run(j.id, j.update_time, j.questions_version, j.deep_version, j.version, j.suggestion, j.brainstorm, j.reason, new Date().toISOString());
	}

	saveJudgment(j: Judgment) {
		this.db.transaction(() => {
			// A re-judgment can change the base verdict without changing the chat or
			// question version. Its earlier follow-up no longer applies.
			this.db.query("delete from deep_judgments where id = ?").run(j.id);
			this.db.query("delete from luna_judgments where id = ?").run(j.id);
			this.db
				.query("insert or replace into judgments (id, update_time, version, content_kind, answers, classified_at) values (?, ?, ?, ?, ?, ?)")
				.run(j.id, j.update_time, j.version, j.content_kind, j.answers, j.classified_at);
		})();
	}

	// Current judgments only: stale ones (chat changed, or questions changed) don't count.
	currentJudgments(version: string): Map<string, Judgment> {
		const rows = this.db
			.query(
				`select j.*, d.answers as deep_answers, d.version as deep_version,
				 l.suggestion as luna_suggestion, l.brainstorm as luna_brainstorm, l.reason as luna_reason, l.version as luna_version
				 from judgments j join conversations c on c.id = j.id
				 left join deep_judgments d on d.id = j.id and d.update_time = j.update_time and d.questions_version = j.version
				 left join luna_judgments l on l.id = j.id and l.update_time = j.update_time and l.questions_version = j.version and l.deep_version = coalesce(d.version, '')
				 where j.update_time = c.update_time and j.version = ?`,
			)
			.all(version) as Judgment[];
		return new Map(rows.map((r) => [r.id, r]));
	}
}
