import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { BrowserCookie } from "./browser-cookies.ts";

const SAFARI_PATHS = [
	"Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies",
	"Library/Cookies/Cookies.binarycookies",
];
const MAC_EPOCH_SECONDS = 978_307_200;

function cString(record: Buffer, offset: number): string {
	if (offset < 0 || offset >= record.length) throw new Error("Invalid Safari cookie string offset.");
	const end = record.indexOf(0, offset);
	if (end < 0) throw new Error("Unterminated Safari cookie string.");
	return record.toString("utf8", offset, end);
}

export function parseSafariCookies(data: Buffer, host: string): BrowserCookie[] {
	if (data.length < 8 || data.toString("ascii", 0, 4) !== "cook") throw new Error("Invalid Safari cookie file.");
	const pages = data.readUInt32BE(4);
	if (pages > (data.length - 8) / 4) throw new Error("Invalid Safari cookie page count.");
	let position = 8 + pages * 4;
	const cookies: BrowserCookie[] = [];
	for (let pageIndex = 0; pageIndex < pages; pageIndex++) {
		const size = data.readUInt32BE(8 + pageIndex * 4);
		if (size < 8 || position + size > data.length) throw new Error("Invalid Safari cookie page size.");
		const page = data.subarray(position, position + size);
		position += size;
		if (page.readUInt32BE(0) !== 256) throw new Error("Invalid Safari cookie page header.");
		const count = page.readUInt32LE(4);
		if (count > (page.length - 8) / 4) throw new Error("Invalid Safari cookie record count.");
		for (let i = 0; i < count; i++) {
			const offset = page.readUInt32LE(8 + i * 4);
			if (offset + 56 > page.length) throw new Error("Invalid Safari cookie record offset.");
			const recordSize = page.readUInt32LE(offset);
			if (recordSize < 56 || offset + recordSize > page.length) throw new Error("Invalid Safari cookie record size.");
			const record = page.subarray(offset, offset + recordSize);
			const domain = cString(record, record.readUInt32LE(16));
			if (domain !== host && domain !== `.${host}`) continue;
			const expiry = record.readDoubleLE(40);
			if (expiry !== 0 && (expiry + MAC_EPOCH_SECONDS) * 1000 <= Date.now()) continue;
			cookies.push({
				name: cString(record, record.readUInt32LE(20)),
				value: cString(record, record.readUInt32LE(28)),
				domain,
			});
		}
	}
	return cookies;
}

export function readSafariCookies(host: string, home = homedir()): BrowserCookie[] {
	for (const relative of SAFARI_PATHS) {
		try {
			return parseSafariCookies(readFileSync(join(home, relative)), host);
		} catch (error) {
			const code = (error as NodeJS.ErrnoException).code;
			if (code === "ENOENT") continue;
			if (code === "EPERM" || code === "EACCES") {
				throw new Error("macOS blocked access to Safari cookies. Grant Full Disk Access to your terminal, then retry.");
			}
			throw error;
		}
	}
	return [];
}
