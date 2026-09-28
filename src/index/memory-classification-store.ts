import type { Database } from "bun:sqlite";

export type MemoryJudgment = {
	id: string;
	input_hash: string;
	version: string;
	system_one: string;
	system_two: string | null;
	classified_at: string;
};

export class MemoryClassificationStore {
	constructor(private readonly db: Database) {
		db.run(`create table if not exists memory_judgments (
			id text primary key, input_hash text not null, version text not null,
			system_one text not null, system_two text, classified_at text not null
		)`);
	}

	get(id: string, hash: string, version: string): MemoryJudgment | null {
		return this.db.query("select * from memory_judgments where id = ? and input_hash = ? and version = ?")
			.get(id, hash, version) as MemoryJudgment | null;
	}

	save(row: MemoryJudgment): void {
		this.db.query("insert or replace into memory_judgments values (?, ?, ?, ?, ?, ?)")
			.run(row.id, row.input_hash, row.version, row.system_one, row.system_two, row.classified_at);
	}
}
