import type { Database } from "bun:sqlite";
import type { IndexedConversation } from "../index/store.ts";
import { RENDER_VERSION } from "../render/transcript.ts";
import { CHUNK_VERSION, transcriptChunks } from "./chunks.ts";
import { EMBEDDING_DIM, type Embedder } from "./embeddings.ts";

export type SearchHit = {
	id: string;
	title: string;
	updated: string;
	archived: boolean;
	score: number;
	snippet: string;
};

type VectorRow = {
	chunk_id: number;
	conversation_id: string;
	title: string;
	update_time: string;
	is_archived: number;
	embedding: Uint8Array;
};

function ftsQuery(query: string): string {
	const terms = query.match(/[\p{L}\p{N}]+/gu);
	if (!terms?.length) throw new Error("Search query needs at least one letter or number.");
	return terms.map((term) => `"${term}"`).join(" AND ");
}

function excerpt(body: string): string {
	return body.replace(/\s+/g, " ").trim().slice(0, 200);
}

export class SearchStore {
	constructor(private readonly db: Database) {
		db.run(`create table if not exists search_chunks (
			id integer primary key, conversation_id text not null, update_time text not null,
			render_version integer not null, chunk_version integer not null,
			chunk_index integer not null, title text not null, body text not null,
			unique(conversation_id, chunk_index)
		)`);
		db.run("create index if not exists search_chunks_conversation on search_chunks(conversation_id)");
		db.run(`create table if not exists search_indexed (
			conversation_id text primary key, update_time text not null,
			render_version integer not null, chunk_version integer not null
		)`);
		db.run(`create table if not exists search_vectors (
			chunk_id integer primary key, model_version text not null, embedding blob not null
		)`);
		db.run("create virtual table if not exists search_fts using fts5(title, body, content='search_chunks', content_rowid='id', tokenize='porter unicode61 remove_diacritics 2')");
		db.run(`create trigger if not exists search_chunks_insert after insert on search_chunks begin
			insert into search_fts(rowid, title, body) values (new.id, new.title, new.body);
		end`);
		db.run(`create trigger if not exists search_chunks_delete after delete on search_chunks begin
			insert into search_fts(search_fts, rowid, title, body) values ('delete', old.id, old.title, old.body);
			delete from search_vectors where chunk_id = old.id;
		end`);
	}

	isCurrent(c: IndexedConversation): boolean {
		return Boolean(this.db.query(`select 1 from search_indexed si
			join search_chunks sc on sc.conversation_id = si.conversation_id and sc.chunk_index = 0
			where si.conversation_id = ? and si.update_time = ?
			and si.render_version = ? and si.chunk_version = ? and sc.title = ?`)
			.get(c.id, c.update_time, RENDER_VERSION, CHUNK_VERSION, c.title));
	}

	replace(c: IndexedConversation, markdown: string): number {
		const chunks = transcriptChunks(markdown);
		this.db.transaction(() => {
			this.db.query("delete from search_chunks where conversation_id = ?").run(c.id);
			const insert = this.db.query(`insert into search_chunks
				(conversation_id, update_time, render_version, chunk_version, chunk_index, title, body)
				values (?, ?, ?, ?, ?, ?, ?)`);
			for (const [i, body] of chunks.entries()) insert.run(c.id, c.update_time, RENDER_VERSION, CHUNK_VERSION, i, c.title, body);
			this.db.query("insert or replace into search_indexed values (?, ?, ?, ?)")
				.run(c.id, c.update_time, RENDER_VERSION, CHUNK_VERSION);
		})();
		return chunks.length;
	}

	prune(): number {
		const obsolete = this.db.query(`select conversation_id from search_indexed
			where conversation_id not in (select id from conversations)`).all() as { conversation_id: string }[];
		if (!obsolete.length) return 0;
		this.db.transaction(() => {
			for (const { conversation_id } of obsolete) {
				this.db.query("delete from search_chunks where conversation_id = ?").run(conversation_id);
				this.db.query("delete from search_indexed where conversation_id = ?").run(conversation_id);
			}
		})();
		return obsolete.length;
	}

	pendingVectors(modelVersion: string, archived: boolean | null = false): { id: number; text: string }[] {
		const scope = archived === null ? null : Number(archived);
		return this.db.query(`select sc.id, sc.title || '\n' || sc.body as text from search_chunks sc
			join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
			left join search_vectors v on v.chunk_id = sc.id
			where sc.render_version = ? and sc.chunk_version = ?
			and (v.chunk_id is null or v.model_version != ?)
			and (? is null or c.is_archived = ?)
			order by sc.id`).all(RENDER_VERSION, CHUNK_VERSION, modelVersion, scope, scope) as { id: number; text: string }[];
	}

	saveVectors(ids: number[], vectors: Float32Array[], modelVersion: string): void {
		if (ids.length !== vectors.length) throw new Error("Embedding batch size mismatch.");
		this.db.transaction(() => {
			const insert = this.db.query("insert or replace into search_vectors values (?, ?, ?)");
			for (let i = 0; i < ids.length; i++) {
				const vector = vectors[i] as Float32Array;
				if (vector.length !== EMBEDDING_DIM) throw new Error(`Expected ${EMBEDDING_DIM} embedding values, got ${vector.length}.`);
				insert.run(ids[i] as number, modelVersion, new Uint8Array(vector.buffer, vector.byteOffset, vector.byteLength));
			}
		})();
	}

	coverage(modelVersion: string, archived: boolean | null = false): { chats: number; indexed: number; chunks: number; embedded: number } {
		const scope = archived === null ? null : Number(archived);
		return this.db.query(`select
			(select count(*) from conversations where (? is null or is_archived = ?)) as chats,
			(select count(*) from search_indexed si join conversations c on c.id = si.conversation_id
				where si.update_time = c.update_time and si.render_version = ? and si.chunk_version = ?
				and (? is null or c.is_archived = ?)) as indexed,
			(select count(*) from search_chunks sc join conversations c on c.id = sc.conversation_id
				where sc.update_time = c.update_time and sc.render_version = ? and sc.chunk_version = ?
				and (? is null or c.is_archived = ?)) as chunks,
			(select count(*) from search_vectors v join search_chunks sc on sc.id = v.chunk_id
				join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
				where v.model_version = ? and sc.render_version = ? and sc.chunk_version = ?
				and (? is null or c.is_archived = ?)) as embedded`)
			.get(
				scope, scope,
				RENDER_VERSION, CHUNK_VERSION, scope, scope,
				RENDER_VERSION, CHUNK_VERSION, scope, scope,
				modelVersion, RENDER_VERSION, CHUNK_VERSION, scope, scope,
			) as { chats: number; indexed: number; chunks: number; embedded: number };
	}

	lexical(query: string, limit: number, archived: boolean | null = false): SearchHit[] {
		const scope = archived === null ? null : Number(archived);
		const rows = this.db.query(`select sc.conversation_id as id, sc.title, c.update_time as updated,
			c.is_archived as archived, bm25(search_fts, 5.0, 1.0) as score,
			snippet(search_fts, 1, '', '', '…', 24) as snippet
			from search_fts join search_chunks sc on sc.id = search_fts.rowid
			join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
			where search_fts match ? and sc.render_version = ? and sc.chunk_version = ?
			and (? is null or c.is_archived = ?)
			order by score limit ?`).all(ftsQuery(query), RENDER_VERSION, CHUNK_VERSION, scope, scope, Math.max(limit * 20, 200)) as
			{ id: string; title: string; updated: string; archived: number; score: number; snippet: string }[];
		const seen = new Set<string>();
		const hits: SearchHit[] = [];
		for (const row of rows) {
			if (seen.has(row.id)) continue;
			seen.add(row.id);
			hits.push({ id: row.id, title: row.title, updated: row.updated, archived: Boolean(row.archived), score: -row.score, snippet: excerpt(row.snippet || row.title) });
			if (hits.length === limit) break;
		}
		return hits;
	}

	semantic(vector: Float32Array, modelVersion: string, limit: number, archived: boolean | null = false): SearchHit[] {
		if (vector.length !== EMBEDDING_DIM) throw new Error(`Expected ${EMBEDDING_DIM} query embedding values.`);
		const scope = archived === null ? null : Number(archived);
		const best = new Map<string, { row: VectorRow; score: number }>();
		const rows = this.db.query(`select sc.id as chunk_id, sc.conversation_id, sc.title,
			c.update_time, c.is_archived, v.embedding from search_vectors v
			join search_chunks sc on sc.id = v.chunk_id
			join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
			where v.model_version = ? and sc.render_version = ? and sc.chunk_version = ?
			and (? is null or c.is_archived = ?)`);
		for (const row of rows.iterate(modelVersion, RENDER_VERSION, CHUNK_VERSION, scope, scope) as Iterable<VectorRow>) {
			if (row.embedding.byteLength !== EMBEDDING_DIM * 4) throw new Error(`Bad embedding for chunk ${row.chunk_id}; run chatgpt search-index.`);
			const values = new DataView(row.embedding.buffer, row.embedding.byteOffset, row.embedding.byteLength);
			let score = 0;
			for (let i = 0; i < EMBEDDING_DIM; i++) score += vector[i] as number * values.getFloat32(i * 4, true);
			const previous = best.get(row.conversation_id);
			if (!previous || score > previous.score) best.set(row.conversation_id, { row, score });
		}
		return [...best.values()].sort((a, b) => b.score - a.score).slice(0, limit).map(({ row, score }) => {
			const chunk = this.db.query("select body from search_chunks where id = ?").get(row.chunk_id) as { body: string };
			return { id: row.conversation_id, title: row.title, updated: row.update_time, archived: Boolean(row.is_archived), score, snippet: excerpt(chunk.body) };
		});
	}
}
