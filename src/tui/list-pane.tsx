import type { Mark, Row } from "./model.ts";
import { displayTitle } from "../index/store.ts";
import { COLOR, SUGGESTION_COLOR } from "./theme.ts";

type Props = {
	rows: Row[];
	start: number;
	height: number;
	width: number;
	selected: number;
	marks: Map<string, Mark>;
	title: string;
};

const pad = (s: string, n: number) => (s.length >= n ? s.slice(0, n) : s + " ".repeat(n - s.length));

// Hand-rolled rather than <select>: each row needs per-column colours and a
// mark, and only the visible slice of a large history should be rendered.
export function ListPane({ rows, start, height, width, selected, marks, title }: Props) {
	const slice = rows.slice(start, start + height);
	// Borders take 2 columns; mark(2) + date(11) + suggestion(9) + topic(15).
	const titleWidth = Math.max(10, width - 2 - 2 - 11 - 9 - 15);
	return (
		<box border borderColor={COLOR.focusBorder} title={title} width={width} flexDirection="column">
			{slice.length === 0 ? <text fg={COLOR.dim}>No conversations match.</text> : null}
			{slice.map((r, i) => {
				const isSelected = start + i === selected;
				const mark = marks.get(r.c.id);
				const suggestion = r.verdict ? `${r.verdict.suggestion}${r.verdict.unsure ? "?" : ""}` : "·";
				return (
					<box key={r.c.id} height={1} backgroundColor={isSelected ? COLOR.selectedBg : undefined}>
						<text wrapMode="none" truncate>
							<span fg={mark === "delete" ? COLOR.markDelete : COLOR.markArchive}>{mark === "delete" ? "D " : mark === "archive" ? "A " : "  "}</span>
							<span fg={COLOR.dim}>{pad(r.c.update_time.slice(0, 10), 11)}</span>
							<span fg={r.verdict ? SUGGESTION_COLOR[r.verdict.suggestion] : COLOR.dim}>{pad(suggestion, 9)}</span>
							<span fg={r.verdict?.brainstorm ? COLOR.accent : COLOR.dim}>{pad(r.verdict?.brainstorm ? `idea:${r.verdict.brainstorm}` : (r.topic ?? ""), 15)}</span>
							<span fg={COLOR.fg}>{pad(displayTitle(r.c), titleWidth)}</span>
						</text>
					</box>
				);
			})}
		</box>
	);
}
