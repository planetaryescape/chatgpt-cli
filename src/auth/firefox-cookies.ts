import { Database } from "bun:sqlite";
import { existsSync } from "node:fs";
import { join } from "node:path";
import type { BrowserCookie } from "./browser-cookies.ts";

export function readFirefoxCookies(profileDir: string, host: string): BrowserCookie[] {
	const source = join(profileDir, "cookies.sqlite");
	if (!existsSync(source)) return [];
	const db = new Database(source, { readonly: true });
	try {
		db.run("pragma busy_timeout = 5000");
		return (db.query(
			"select host, name, value from moz_cookies where host in (?, ?) and (expiry = 0 or expiry > ?) and coalesce(originAttributes, '') = ''"
		).all(host, `.${host}`, Math.floor(Date.now() / 1000)) as { host: string; name: string; value: string }[])
			.map((row) => ({ name: row.name, value: row.value, domain: row.host }));
	} finally {
		db.close();
	}
}
