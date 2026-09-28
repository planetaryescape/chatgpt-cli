import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { findSecret } from "../auth/secrets.ts";
import { openAIText } from "./model-api.ts";

const NO_TOOLS = ["apps", "browser_use", "computer_use", "image_generation", "plugins", "multi_agent", "view_image", "unified_exec", "shell_tool"];

export async function askLuna<T>(instructions: string, input: unknown, schema: object): Promise<T> {
	if (findSecret("OPENAI_API_KEY")) {
		return JSON.parse((await openAIText(instructions, JSON.stringify(input), schema)).text) as T;
	}
	if (!Bun.which("codex")) throw new Error("codex is required for gpt-6-luna.");
	const dir = mkdtempSync(join(tmpdir(), "chatgpt-cli-luna-"));
	try {
		const schemaFile = join(dir, "schema.json");
		const outputFile = join(dir, "response.json");
		writeFileSync(schemaFile, JSON.stringify(schema));
		const proc = Bun.spawn([
			"codex", "exec", "-m", "gpt-6-luna", "-c", 'model_reasoning_effort="medium"',
			"--skip-git-repo-check", "--ephemeral", "--sandbox", "read-only", "--ignore-user-config",
			...NO_TOOLS.flatMap((name) => ["--disable", name]),
			"-C", dir, "--output-schema", schemaFile, "-o", outputFile, instructions,
		], { cwd: dir, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
		proc.stdin.write(JSON.stringify(input));
		proc.stdin.end();
		const [stdout, stderr, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
		if (code !== 0) throw new Error(`gpt-6-luna exited ${code}: ${(stderr || stdout).trim().slice(-300)}`);
		return JSON.parse(readFileSync(outputFile, "utf8")) as T;
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
}
