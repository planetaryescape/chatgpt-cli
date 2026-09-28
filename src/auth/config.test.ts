import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readConfig, setConfigKey } from "./config.ts";

test("configured API keys are private, replaceable, and removable", () => {
	const dir = mkdtempSync(join(tmpdir(), "chatgpt-cli-config-test-"));
	try {
		const path = join(dir, "nested", "config.json");
		setConfigKey("jev", "  jev-secret  ", path);
		setConfigKey("openai", "openai-secret", path);
		expect(readConfig(path)).toEqual({ jev: "jev-secret", openai: "openai-secret" });
		expect(statSync(path).mode & 0o777).toBe(0o600);
		setConfigKey("jev", null, path);
		expect(readConfig(path)).toEqual({ openai: "openai-secret" });
		expect(readFileSync(path, "utf8")).not.toContain("jev-secret");
	} finally { rmSync(dir, { recursive: true, force: true }); }
});
