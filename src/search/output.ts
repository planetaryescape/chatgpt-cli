export type SearchOutputFormat = "json" | "csv" | "table" | "ids";

export type SearchResult = {
	id: string;
	title: string;
	updated: string;
	archived: boolean;
	score: number | null;
	snippet: string;
};

const CSV_HEADERS = ["id", "title", "updated", "archived", "score", "snippet"] as const;

function csvCell(value: string | number | boolean | null): string {
	return `"${String(value ?? "").replaceAll('"', '""')}"`;
}

function tableCell(value: string, width: number): string {
	const text = value.replace(/\s+/g, " ").trim();
	if (Bun.stringWidth(text) <= width) return text + " ".repeat(width - Bun.stringWidth(text));
	let clipped = "";
	for (const character of text) {
		if (Bun.stringWidth(clipped + character) > width - 1) break;
		clipped += character;
	}
	const result = `${clipped}…`;
	return result + " ".repeat(width - Bun.stringWidth(result));
}

function searchTable(results: SearchResult[]): string {
	const widths = [12, 10, 1, 36, 50] as const;
	const row = (cells: string[]) => cells.map((cell, i) => tableCell(cell, widths[i] as number)).join("  ");
	return [
		row(["id", "updated", "A", "title", "snippet"]),
		row(widths.map((width) => "─".repeat(width))),
		...results.map((hit) => row([hit.id.slice(0, 12), hit.updated.slice(0, 10), hit.archived ? "A" : "", hit.title, hit.snippet])),
	].join("\n");
}

export function formatSearchResults(results: SearchResult[], format?: SearchOutputFormat): string {
	if (format === "json") return JSON.stringify(results, null, 2);
	if (format === "ids") return results.map((hit) => hit.id).join("\n");
	if (format === "csv") {
		return [CSV_HEADERS.join(","), ...results.map((hit) =>
			CSV_HEADERS.map((key) => csvCell(hit[key])).join(","))].join("\n");
	}
	if (format === "table") return searchTable(results);
	return results.map((hit) => `${hit.id}  ${hit.updated.slice(0, 10)}  ${hit.archived ? "A" : " "}  ${hit.title}\n    ${hit.snippet}`).join("\n");
}
