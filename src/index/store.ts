import { Database } from "bun:sqlite";
import { mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ConversationSummary } from "../api/conversations.ts";

export type IndexedConversation = {
	id: string;
	title: string;
	local_title?: string | null;
	create_time: string;
	update_time: string;
	is_archived: 0 | 1;
	pinned: 0 | 1;
	project_id: string | null;
};

export const LOCAL_TITLE_VERSION = 2;
export const displayTitle = (c: IndexedConversation): string => c.local_title || c.title;

export type Filter = {
	archived?: boolean;
	updatedBefore?: Date;
	updatedAfter?: Date;
	titleMatch?: RegExp;
	includePinned?: boolean;
};

const DATA_DIR = join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local/share"), "chatgpt-cli");

export class ConversationIndex {
	readonly db: Database;

	constructor(path = join(DATA_DIR, "index.db")) {
		mkdirSync(DATA_DIR, { recursive: true });
		this.db = new Database(path, { create: true });
		this.db.run("pragma busy_timeout = 5000");
		this.db.run(`create table if not exists conversations (
			id text primary key,
			title text not null,
			create_time text not null,
			update_time text not null,
			is_archived integer not null,
			pinned integer not null,
			project_id text
		)`);
		this.db.run("create table if not exists meta (key text primary key, value text not null)");
		this.db.run(`create table if not exists local_titles (
			id text primary key, update_time text not null, version integer not null,
			source text not null check(source in ('luna', 'manual')),
			title text not null, theme text not null default '', updated_at text not null
		)`);
	}

	// Full replace so conversations deleted elsewhere drop out of the index.
	private row(c: ConversationSummary) {
		return {
			$id: c.id,
			$title: c.title ?? "(untitled)",
			$create_time: c.create_time,
			$update_time: c.update_time,
			$is_archived: c.is_archived ? 1 : 0,
			$pinned: c.pinned_time ? 1 : 0,
			// Project conversations carry the project's gizmo id (g-p-…).
			$project_id: c.gizmo_id?.startsWith("g-p-") ? c.gizmo_id : null,
		};
	}

	private upsertStatement() {
		return this.db.prepare(
			"insert or replace into conversations values ($id, $title, $create_time, $update_time, $is_archived, $pinned, $project_id)",
		);
	}

	private markSynced() {
		this.db.run("insert or replace into meta values ('synced_at', ?)", [new Date().toISOString()]);
	}

	// Full replace so conversations deleted elsewhere drop out of the index.
	replaceAll(items: ConversationSummary[]) {
		const upsert = this.upsertStatement();
		this.db.transaction(() => {
			this.db.run("delete from conversations");
			for (const c of items) upsert.run(this.row(c));
			this.markSynced();
		})();
	}

	// Newest update_time among active chats: a delta sync reads the active list
	// from the top until it reaches this.
	activeWatermark(): string | null {
		const row = this.db.query("select max(update_time) as t from conversations where is_archived = 0").get() as { t: string | null };
		return row.t;
	}

	// Archiving doesn't bump update_time (observed 2026-09-27), so archive state
	// comes from the complete archived list rather than the delta.
	applyDelta(changedActive: ConversationSummary[], allArchived: ConversationSummary[]) {
		const upsert = this.upsertStatement();
		this.db.transaction(() => {
			for (const c of changedActive) upsert.run(this.row(c));
			for (const c of allArchived) upsert.run(this.row(c));
			this.markSynced();
		})();
	}

	archivedIds(): string[] {
		return (this.db.query("select id from conversations where is_archived = 1").all() as { id: string }[]).map((r) => r.id);
	}

	syncedAt(): Date | null {
		const row = this.db.query("select value from meta where key = 'synced_at'").get() as { value: string } | null;
		return row ? new Date(row.value) : null;
	}

	query(filter: Filter): IndexedConversation[] {
		const where: string[] = [];
		const params: (string | number)[] = [];
		if (filter.archived !== undefined) {
			where.push("c.is_archived = ?");
			params.push(filter.archived ? 1 : 0);
		}
		if (!filter.includePinned) where.push("c.pinned = 0");
		if (filter.updatedBefore) {
			where.push("c.update_time < ?");
			params.push(filter.updatedBefore.toISOString());
		}
		if (filter.updatedAfter) {
			where.push("c.update_time >= ?");
			params.push(filter.updatedAfter.toISOString());
		}
		const sql = `select c.*, case when l.source = 'manual' or (l.update_time = c.update_time and l.version = ${LOCAL_TITLE_VERSION})
			then l.title end as local_title from conversations c left join local_titles l on l.id = c.id
			${where.length ? `where ${where.join(" and ")}` : ""} order by c.update_time desc`;
		let rows = this.db.query(sql).all(...params) as IndexedConversation[];
		// SQLite has no regex; titles are small enough to filter here.
		if (filter.titleMatch) rows = rows.filter((r) => filter.titleMatch?.test(displayTitle(r)) || filter.titleMatch?.test(r.title));
		return rows;
	}

	get(idOrPrefix: string): IndexedConversation[] {
		return this.db.query(`select c.*, case when l.source = 'manual' or (l.update_time = c.update_time and l.version = ${LOCAL_TITLE_VERSION})
			then l.title end as local_title from conversations c left join local_titles l on l.id = c.id where c.id like ?`).all(`${idOrPrefix}%`) as IndexedConversation[];
	}

	localTitle(c: IndexedConversation): { title: string; theme: string; source: "luna" | "manual" } | null {
		return this.db.query(`select title, theme, source from local_titles where id = ? and (source = 'manual' or (update_time = ? and version = ?))`)
			.get(c.id, c.update_time, LOCAL_TITLE_VERSION) as { title: string; theme: string; source: "luna" | "manual" } | null;
	}

	setLocalTitle(c: IndexedConversation, title: string, theme = "", source: "luna" | "manual" = "manual") {
		const clean = title.trim().replace(/\s+/g, " ");
		if (!clean || clean.length > 100) throw new Error("Local title must be 1–100 characters.");
		this.db.query("insert or replace into local_titles values (?, ?, ?, ?, ?, ?, ?)")
			.run(c.id, c.update_time, LOCAL_TITLE_VERSION, source, clean, theme, new Date().toISOString());
	}

	setArchived(id: string, archived: boolean) {
		this.db.run("update conversations set is_archived = ? where id = ?", [archived ? 1 : 0, id]);
	}

	setProject(id: string, projectId: string | null) {
		this.db.run("update conversations set project_id = ? where id = ?", [projectId, id]);
	}

	rename(id: string, title: string) {
		this.db.run("update conversations set title = ? where id = ?", [title, id]);
	}

	remove(id: string) {
		this.db.transaction(() => {
			this.db.run("delete from conversations where id = ?", [id]);
			this.db.run("delete from local_titles where id = ?", [id]);
		})();
	}
}
