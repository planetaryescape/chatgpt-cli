import { expect, test } from "bun:test";
import { formatSearchResults, type SearchResult } from "./output.ts";

const hit: SearchResult = {
	id: "00000000-0000-4000-8000-000000000005",
	title: 'One, "Two"',
	updated: "2026-07-21T12:34:56Z",
	archived: false,
	score: 0.875,
	snippet: "A line with a comma, a quote \" and a newline\ninside.",
};

test("JSON search output is a parseable array with the same fields when empty", () => {
	expect(JSON.parse(formatSearchResults([hit], "json"))).toEqual([hit]);
	expect(JSON.parse(formatSearchResults([], "json"))).toEqual([]);
});

test("ids output has one full id per line and no header", () => {
	expect(formatSearchResults([hit, { ...hit, id: "second" }], "ids")).toBe(`${hit.id}\nsecond`);
	expect(formatSearchResults([], "ids")).toBe("");
});

test("CSV quotes commas, quotes and newlines without losing fields", () => {
	expect(formatSearchResults([hit], "csv")).toBe(
		'id,title,updated,archived,score,snippet\n"00000000-0000-4000-8000-000000000005","One, ""Two""","2026-07-21T12:34:56Z","false","0.875","A line with a comma, a quote "" and a newline\ninside."',
	);
	expect(formatSearchResults([], "csv")).toBe("id,title,updated,archived,score,snippet");
});

test("table output has aligned, compact rows and default text keeps full ids", () => {
	const lines = formatSearchResults([hit], "table").split("\n");
	expect(lines).toHaveLength(3);
	expect(lines.map((line) => Bun.stringWidth(line))).toEqual([117, 117, 117]);
	expect(lines[2]).toContain("00000000-000");
	expect(lines[2]).toContain("newline insi…");
	expect(formatSearchResults([hit])).toContain(hit.id);
});
