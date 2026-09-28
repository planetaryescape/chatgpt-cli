import { expect, test } from "bun:test";
import { REFERENCE_PATH, renderReference } from "./gen-cli-reference.ts";

test("docs/reference/cli.md matches the CLI's --help output", async () => {
	const onDisk = await Bun.file(REFERENCE_PATH).text();
	// On failure: run `bun run docs:cli` and commit the result.
	expect(onDisk).toBe(renderReference());
}, 60_000);
