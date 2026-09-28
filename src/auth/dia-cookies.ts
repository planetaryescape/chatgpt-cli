import { Database } from "bun:sqlite";
import { createDecipheriv, pbkdf2Sync } from "node:crypto";
import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

const DIA_USER_DATA = join(homedir(), "Library/Application Support/Dia/User Data");
const KEYCHAIN_SERVICE = "Dia Safe Storage";

export type BrowserCookie = { name: string; value: string; domain: string };

// Chromium on macOS encrypts cookie values with AES-128-CBC. The key is derived
// from a random password it keeps in the login Keychain.
function readKeychainPassword(): string {
	const proc = Bun.spawnSync(["security", "find-generic-password", "-w", "-s", KEYCHAIN_SERVICE]);
	if (proc.exitCode !== 0) {
		throw new Error(
			`Could not read "${KEYCHAIN_SERVICE}" from the Keychain. Allow access when macOS prompts.\n${proc.stderr.toString().trim()}`,
		);
	}
	return proc.stdout.toString().trim();
}

function decrypt(encrypted: Uint8Array, key: Buffer, stripHostHash: boolean): string {
	const prefix = Buffer.from(encrypted.subarray(0, 3)).toString();
	if (prefix !== "v10") throw new Error(`Unsupported cookie encryption version "${prefix}"`);
	const decipher = createDecipheriv("aes-128-cbc", key, Buffer.alloc(16, " "));
	const plain = Buffer.concat([decipher.update(encrypted.subarray(3)), decipher.final()]);
	// Cookie DB schema 24+ prefixes the plaintext with SHA-256(host_key).
	return (stripHostHash ? plain.subarray(32) : plain).toString("utf8");
}

export function readDiaCookies(hostSuffix: string, profile = "Default"): BrowserCookie[] {
	// Dia holds a lock on the live DB, so read a copy.
	const dir = mkdtempSync(join(tmpdir(), "chatgpt-cli-"));
	try {
		const copy = join(dir, "Cookies");
		copyFileSync(join(DIA_USER_DATA, profile, "Cookies"), copy);
		const db = new Database(copy, { readonly: true });
		try {
			const meta = db.query("select value from meta where key = 'version'").get() as { value: string } | null;
			const stripHostHash = Number(meta?.value ?? 0) >= 24;
			const rows = db
				.query("select host_key, name, encrypted_value from cookies where host_key like ?")
				.all(`%${hostSuffix}`) as { host_key: string; name: string; encrypted_value: Uint8Array }[];
			if (rows.length === 0) return [];
			const key = pbkdf2Sync(readKeychainPassword(), "saltysalt", 1003, 16, "sha1");
			return rows.map((r) => ({ name: r.name, domain: r.host_key, value: decrypt(r.encrypted_value, key, stripHostHash) }));
		} finally {
			db.close();
		}
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
}
