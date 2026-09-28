import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

export type Provider = "jev" | "openai" | "anthropic";
type Config = Partial<Record<Provider, string>>;

export function configPath(): string {
	return join(process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config"), "chatgpt-cli", "config.json");
}

export function readConfig(path = configPath()): Config {
	let raw: string;
	try { raw = readFileSync(path, "utf8"); }
	catch (err) {
		if ((err as NodeJS.ErrnoException).code === "ENOENT") return {};
		throw err;
	}
	let parsed: unknown;
	try { parsed = JSON.parse(raw); }
	catch { throw new Error(`Invalid JSON in ${path}.`); }
	if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error(`Invalid config in ${path}.`);
	const config: Config = {};
	for (const name of ["jev", "openai", "anthropic"] as const) {
		const value = (parsed as Record<string, unknown>)[name];
		if (value !== undefined && typeof value !== "string") throw new Error(`Invalid ${name} key in ${path}.`);
		if (value?.trim()) config[name] = value.trim();
	}
	return config;
}

export function setConfigKey(provider: Provider, key: string | null, path = configPath()): void {
	const config = readConfig(path);
	if (key) config[provider] = key.trim();
	else delete config[provider];
	const dir = dirname(path);
	mkdirSync(dir, { recursive: true, mode: 0o700 });
	const temp = join(dir, `.config-${process.pid}-${crypto.randomUUID()}.tmp`);
	try {
		writeFileSync(temp, `${JSON.stringify(config, null, 2)}\n`, { mode: 0o600, flag: "wx" });
		renameSync(temp, path);
		chmodSync(path, 0o600);
	} finally {
		rmSync(temp, { force: true });
	}
}

export async function readKeyFromStdin(): Promise<string> {
	if (!process.stdin.isTTY) return (await Bun.stdin.text()).trim();
	const input = process.stdin;
	const output = process.stderr;
	return await new Promise<string>((resolve, reject) => {
		let value = "";
		input.setRawMode(true);
		input.resume();
		const finish = (error?: Error) => {
			input.off("data", onData);
			input.setRawMode(false);
			input.pause();
			output.write("\n");
			if (error) reject(error);
			else resolve(value.trim());
		};
		const onData = (chunk: Buffer) => {
			for (const char of chunk.toString()) {
				if (char === "\r" || char === "\n") return finish();
				if (char === "\u0003") return finish(new Error("Cancelled."));
				if (char === "\u007f") value = value.slice(0, -1);
				else value += char;
			}
		};
		input.on("data", onData);
	});
}
