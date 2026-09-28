import { Database } from "bun:sqlite";
import { createDecipheriv, pbkdf2Sync } from "node:crypto";
import { existsSync } from "node:fs";
import { join } from "node:path";
import type { BrowserCookie } from "./browser-cookies.ts";

function keychainPassword(service: string): string {
	const proc = Bun.spawnSync(["security", "find-generic-password", "-w", "-s", service]);
	if (proc.exitCode !== 0) {
		throw new Error(`Could not read "${service}" from the Keychain. Allow access when macOS prompts.\n${proc.stderr.toString().trim()}`);
	}
	return proc.stdout.toString().trim();
}

export function decryptChromiumCookie(encrypted: Uint8Array, key: Buffer, stripHostHash: boolean): string {
	const prefix = Buffer.from(encrypted.subarray(0, 3)).toString();
	if (prefix !== "v10") throw new Error(`Unsupported cookie encryption version "${prefix}"`);
	const decipher = createDecipheriv("aes-128-cbc", key, Buffer.alloc(16, " "));
	const plain = Buffer.concat([decipher.update(encrypted.subarray(3)), decipher.final()]);
	// Cookie DB schema 24+ prefixes the plaintext with SHA-256(host_key).
	return (stripHostHash ? plain.subarray(32) : plain).toString("utf8");
}

export function readChromiumCookies(
	userData: string,
	profile: string,
	keychainService: string,
	host: string,
	readPassword = keychainPassword,
): BrowserCookie[] {
	const profileDir = join(userData, profile);
	const source = [join(profileDir, "Network", "Cookies"), join(profileDir, "Cookies")].find(existsSync);
	if (!source) return [];
	// A read-only SQLite connection sees uncheckpointed WAL writes from an active browser.
	const db = new Database(source, { readonly: true });
	try {
		db.run("pragma busy_timeout = 5000");
		const meta = db.query("select value from meta where key = 'version'").get() as { value: string } | null;
		const stripHostHash = Number(meta?.value ?? 0) >= 24;
		const rows = db.query(
			"select host_key, name, value, encrypted_value, expires_utc from cookies where host_key in (?, ?)"
		).all(host, `.${host}`) as {
			host_key: string; name: string; value: string; encrypted_value: Uint8Array; expires_utc: number;
		}[];
		const unexpired = rows.filter((row) => !row.expires_utc || row.expires_utc > (Date.now() + 11_644_473_600_000) * 1000);
		if (!unexpired.some((row) => row.name.startsWith("__Secure-next-auth.session-token"))) return [];
		let key: Buffer | undefined;
		return unexpired.map((row) => {
			if (row.value) return { name: row.name, value: row.value, domain: row.host_key };
			key ??= pbkdf2Sync(readPassword(keychainService), "saltysalt", 1003, 16, "sha1");
			return { name: row.name, value: decryptChromiumCookie(row.encrypted_value, key, stripHostHash), domain: row.host_key };
		});
	} finally {
		db.close();
	}
}
