import { Database } from "bun:sqlite";
import { expect, test } from "bun:test";
import { createCipheriv, createHash, pbkdf2Sync } from "node:crypto";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { browserForBundleId, readBrowserSession } from "./browser-cookies.ts";
import { readChromiumCookies } from "./chromium-cookies.ts";
import { readFirefoxCookies } from "./firefox-cookies.ts";
import { parseSafariCookies } from "./safari-cookies.ts";

const TOKEN = "__Secure-next-auth.session-token";

function tempHome(run: (home: string) => void): void {
	const home = mkdtempSync(join(tmpdir(), "chatgpt-auth-test-"));
	try { run(home); }
	finally { rmSync(home, { recursive: true, force: true }); }
}

function chromeFixture(home: string, profile: string, value: string, encrypted = false): string {
	const root = join(home, "Library/Application Support/Google/Chrome");
	const path = join(root, profile, "Cookies");
	mkdirSync(dirname(path), { recursive: true });
	const db = new Database(path);
	db.run("create table meta (key text, value text)");
	db.run("insert into meta values ('version', '24')");
	db.run("create table cookies (host_key text, name text, value text, encrypted_value blob, expires_utc integer)");
	let cipher = Buffer.alloc(0);
	if (encrypted) {
		const key = pbkdf2Sync("test-password", "saltysalt", 1003, 16, "sha1");
		const encryptor = createCipheriv("aes-128-cbc", key, Buffer.alloc(16, " "));
		cipher = Buffer.concat([Buffer.from("v10"), encryptor.update(Buffer.concat([createHash("sha256").update(".chatgpt.com").digest(), Buffer.from(value)])), encryptor.final()]);
	}
	db.query("insert into cookies values (?, ?, ?, ?, ?)").run(".chatgpt.com", TOKEN, encrypted ? "" : value, cipher, 0);
	db.query("insert into cookies values (?, ?, ?, ?, ?)").run("evilchatgpt.com", TOKEN, "wrong-domain", Buffer.alloc(0), 0);
	db.close();
	return root;
}

function firefoxFixture(home: string): string {
	const profile = join(home, "Library/Application Support/Firefox/Profiles/test.default");
	mkdirSync(profile, { recursive: true });
	const db = new Database(join(profile, "cookies.sqlite"));
	db.run("create table moz_cookies (host text, name text, value text, expiry integer, originAttributes text)");
	const future = Math.floor(Date.now() / 1000) + 3600;
	db.query("insert into moz_cookies values (?, ?, ?, ?, ?)").run(".chatgpt.com", TOKEN, "firefox-session", future, "");
	db.query("insert into moz_cookies values (?, ?, ?, ?, ?)").run(".chatgpt.com", TOKEN, "container-session", future, "^userContextId=1");
	db.query("insert into moz_cookies values (?, ?, ?, ?, ?)").run(".chatgpt.com", "expired", "old", 1, "");
	db.close();
	return profile;
}

function safariRecord(domain: string, value: string): Buffer {
	const strings = [domain, TOKEN, "/", value].map((part) => Buffer.from(`${part}\0`));
	const record = Buffer.alloc(56 + strings.reduce((sum, part) => sum + part.length, 0));
	record.writeUInt32LE(record.length, 0);
	record.writeUInt32LE(5, 8);
	let offset = 56;
	for (let i = 0; i < strings.length; i++) {
		record.writeUInt32LE(offset, 16 + i * 4);
		strings[i]!.copy(record, offset);
		offset += strings[i]!.length;
	}
	record.writeDoubleLE(Date.now() / 1000 - 978_307_200 + 3600, 40);
	return record;
}

test("maps macOS default browser bundle IDs", () => {
	expect(browserForBundleId("com.google.Chrome")).toBe("chrome");
	expect(browserForBundleId("com.apple.Safari")).toBe("safari");
	expect(browserForBundleId("company.thebrowser.dia")).toBe("dia");
	expect(browserForBundleId("company.thebrowser.Browser")).toBe("arc");
	expect(browserForBundleId("unknown.browser")).toBeUndefined();
});

test("decrypts Chrome session cookies and ignores lookalike domains", () => tempHome((home) => {
	const root = chromeFixture(home, "Default", "chrome-session", true);
	const cookies = readChromiumCookies(root, "Default", "Chrome Safe Storage", "chatgpt.com", () => "test-password");
	expect(cookies).toEqual([{ name: TOKEN, value: "chrome-session", domain: ".chatgpt.com" }]);
}));

test("reads a browser login still in SQLite's write-ahead log", () => tempHome((home) => {
	const root = chromeFixture(home, "Default", "old-session");
	const db = new Database(join(root, "Default", "Cookies"));
	try {
		db.run("pragma journal_mode = WAL");
		db.query("update cookies set value = ? where host_key = ?").run("fresh-session", ".chatgpt.com");
		expect(readChromiumCookies(root, "Default", "Chrome Safe Storage", "chatgpt.com")[0]?.value).toBe("fresh-session");
	} finally {
		db.close();
	}
}));

test("reads only the default Firefox container and unexpired cookies", () => tempHome((home) => {
	const cookies = readFirefoxCookies(firefoxFixture(home), "chatgpt.com");
	expect(cookies).toEqual([{ name: TOKEN, value: "firefox-session", domain: ".chatgpt.com" }]);
}));

test("parses Safari cookies without including other domains", () => {
	const records = [safariRecord(".chatgpt.com", "safari-session"), safariRecord("other.example", "wrong-domain")];
	const page = Buffer.alloc(16 + records.reduce((sum, record) => sum + record.length, 0));
	page.writeUInt32BE(256, 0);
	page.writeUInt32LE(records.length, 4);
	let offset = 16;
	for (let i = 0; i < records.length; i++) {
		page.writeUInt32LE(offset, 8 + i * 4);
		records[i]!.copy(page, offset);
		offset += records[i]!.length;
	}
	const file = Buffer.alloc(12 + page.length);
	file.write("cook", 0);
	file.writeUInt32BE(1, 4);
	file.writeUInt32BE(page.length, 8);
	page.copy(file, 12);
	expect(parseSafariCookies(file, "chatgpt.com")).toEqual([{ name: TOKEN, value: "safari-session", domain: ".chatgpt.com" }]);
});

test.skipIf(process.platform !== "darwin")("chooses the default browser and the most recently used Chrome profile", () => tempHome((home) => {
	firefoxFixture(home);
	const chrome = chromeFixture(home, "Default", "old-profile");
	chromeFixture(home, "Profile 1", "current-profile");
	writeFileSync(join(chrome, "Local State"), JSON.stringify({ profile: { last_used: "Profile 1" } }));
	expect(readBrowserSession({}, home, "firefox").browser).toBe("firefox");
	expect(readBrowserSession({ browser: "chrome" }, home).cookies[0]?.value).toBe("current-profile");
	expect(readBrowserSession({ browser: "chrome", profile: "Default" }, home).cookies[0]?.value).toBe("old-profile");
}));

test.skipIf(process.platform !== "darwin")("does not switch accounts by falling back to another browser", () => tempHome((home) => {
	chromeFixture(home, "Default", "chrome-session");
	expect(() => readBrowserSession({}, home, "firefox")).toThrow("No ChatGPT session in firefox");
}));
