import type { ScrollBoxRenderable } from "@opentui/core";
import { useKeyboard, useRenderer, useTerminalDimensions } from "@opentui/react";
import { useMemo, useRef, useState } from "react";
import type { ChatGPTClient } from "../api/client.ts";
import { copyToClipboard } from "../commands/export.ts";
import { performAction } from "../commands/mutate.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import { displayTitle, type ConversationIndex } from "../index/store.ts";
import { ListPane } from "./list-pane.tsx";
import { AGE_CYCLE, BRAINSTORM_CYCLE, cycle, INITIAL_VIEW, loadRows, type Mark, type Row, TOPIC_CYCLE, type View, visibleRows, windowStart } from "./model.ts";
import { PreviewPane } from "./preview-pane.tsx";
import { COLOR } from "./theme.ts";
import { useTranscript } from "./use-transcript.ts";

type Props = { client: ChatGPTClient; index: ConversationIndex; store: ClassificationStore };

type Mode = { kind: "browse" } | { kind: "filter" } | { kind: "help" } | { kind: "confirm" } | { kind: "title"; id: string } | { kind: "applying"; done: number; total: number };

const SUGGESTION_KEYS: Record<string, View["suggestion"]> = { "1": "all", "2": "delete", "3": "archive", "4": "keep", "5": "unjudged", "6": "unsure" };

const HELP = `j/k ↑/↓  move            g/G  top/bottom       ctrl-d/u  page
J/K      scroll preview  /    filter titles    esc  clear filter
1-6      all · delete · archive · keep · unjudged · unsure
t/T      next/previous topic                    A    archived view
y/Y      older than: any · 30d · 6m · 1y · 2y · 3y
b/B      brainstorms: off · any · writing · sermon · product · other
d / a    mark delete / archive (again to unmark)   u  unmark
enter    take suggestion and move on             x    apply marks
c        copy transcript    o  open in browser  r    reload data
n        edit local title (ChatGPT title stays unchanged)
q        quit               ?  close help`;

export function App({ client, index, store }: Props) {
	const renderer = useRenderer();
	const { width, height } = useTerminalDimensions();
	const [rows, setRows] = useState<Row[]>(() => loadRows(index, store));
	const [view, setView] = useState<View>(INITIAL_VIEW);
	const [selected, setSelected] = useState(0);
	const [start, setStart] = useState(0);
	const [marks, setMarks] = useState(() => new Map<string, Mark>());
	const [mode, setMode] = useState<Mode>({ kind: "browse" });
	const [confirmText, setConfirmText] = useState("");
	const [titleText, setTitleText] = useState("");
	const [status, setStatus] = useState("");
	const [quitArmed, setQuitArmed] = useState(false);
	const scrollRef = useRef<ScrollBoxRenderable | null>(null);

	const visible = useMemo(() => visibleRows(rows, view), [rows, view]);
	const current = visible[Math.min(selected, visible.length - 1)];
	const transcript = useTranscript(client, store, current?.c);

	// Keys can arrive faster than React re-renders (key repeat), so the handler
	// reads live values from refs instead of this render's closure.
	const live = useRef({ selected, mode, visible, view, rows });
	live.current.visible = visible;
	live.current.view = view;
	live.current.rows = rows;
	const setModeNow = (m: Mode) => {
		live.current.mode = m;
		setMode(m);
	};

	// Header (1) + footer (1) + list borders (2).
	const listHeight = Math.max(1, height - 4);
	const listWidth = Math.max(60, Math.floor(width * 0.5));

	const select = (next: number) => {
		const total = live.current.visible.length;
		const clamped = Math.max(0, Math.min(next, total - 1));
		live.current.selected = clamped;
		setSelected(clamped);
		setStart((s) => windowStart(clamped, s, listHeight, total));
		scrollRef.current?.scrollTo(0);
	};
	const moveBy = (delta: number) => select(live.current.selected + delta);

	const changeView = (patch: (v: View) => Partial<View>) => {
		const next = { ...live.current.view, ...patch(live.current.view) };
		live.current.view = next;
		live.current.visible = visibleRows(live.current.rows, next);
		setView(next);
		live.current.selected = 0;
		setSelected(0);
		setStart(0);
	};

	const toggleMark = (id: string, mark: Mark | null) =>
		setMarks((m) => {
			const next = new Map(m);
			if (mark === null || next.get(id) === mark) next.delete(id);
			else next.set(id, mark);
			return next;
		});

	const markCounts = () => {
		let del = 0;
		for (const m of marks.values()) if (m === "delete") del++;
		return { del, arch: marks.size - del };
	};

	const apply = async () => {
		const entries = [...marks.entries()];
		const failures: string[] = [];
		setModeNow({ kind: "applying", done: 0, total: entries.length });
		for (const [i, [id, action]] of entries.entries()) {
			try {
				await performAction(client, index, action, id);
			} catch (err) {
				failures.push(err instanceof Error ? err.message : String(err));
			}
			setModeNow({ kind: "applying", done: i + 1, total: entries.length });
			await Bun.sleep(250);
		}
		setMarks(new Map());
		setRows(loadRows(index, store));
		select(0);
		setModeNow({ kind: "browse" });
		setStatus(failures.length ? `${failures.length} failed: ${failures[0]}` : `Applied ${entries.length} change(s).`);
	};

	useKeyboard((key) => {
		if (key.ctrl && key.name === "c") return renderer.destroy();
		const mode = live.current.mode;
		if (mode.kind === "applying") return;
		if (mode.kind === "help") return setModeNow({ kind: "browse" });
		if (mode.kind === "filter") {
			if (key.name === "escape") changeView(() => ({ query: "" }));
			if (key.name === "escape" || key.name === "return") setModeNow({ kind: "browse" });
			return;
		}
		if (mode.kind === "confirm") {
			if (key.name === "escape") {
				setModeNow({ kind: "browse" });
				setConfirmText("");
			}
			return;
		}
		if (mode.kind === "title") {
			if (key.name === "escape") setModeNow({ kind: "browse" });
			return;
		}

		if (key.name !== "q") setQuitArmed(false);
		const name = key.name;
		const shifted = key.shift || (name.length === 1 && name !== name.toLowerCase());
		switch (true) {
			case name === "q": {
				if (marks.size && !quitArmed) {
					setQuitArmed(true);
					setStatus(`${marks.size} unapplied mark(s). Press q again to quit without applying.`);
					return;
				}
				return renderer.destroy();
			}
			case name === "?":
				return setModeNow({ kind: "help" });
			case name === "/":
				return setModeNow({ kind: "filter" });
			case name === "escape":
				return changeView(() => ({ query: "" }));
			case (name === "j" && !shifted) || name === "down":
				return moveBy(1);
			case (name === "k" && !shifted) || name === "up":
				return moveBy(-1);
			case name === "j" && shifted:
				return scrollRef.current?.scrollBy(3);
			case name === "k" && shifted:
				return scrollRef.current?.scrollBy(-3);
			case name === "space":
				return scrollRef.current?.scrollBy(1, "viewport");
			case name === "g" && !shifted:
				return select(0);
			case name === "g" && shifted:
				return select(live.current.visible.length - 1);
			case key.ctrl && name === "d":
			case name === "pagedown":
				return moveBy(listHeight);
			case key.ctrl && name === "u":
			case name === "pageup":
				return moveBy(-listHeight);
			case name in SUGGESTION_KEYS:
				return changeView(() => ({ suggestion: SUGGESTION_KEYS[name] }));
			case name === "t":
				return changeView((v) => ({ topic: cycle(TOPIC_CYCLE, v.topic, shifted ? -1 : 1) }));
			case name === "a" && shifted:
				return changeView((v) => ({ archived: !v.archived }));
			case name === "y":
				return changeView((v) => ({ olderThan: cycle(AGE_CYCLE, v.olderThan, shifted ? -1 : 1) }));
			case name === "b":
				return changeView((v) => ({ brainstorm: cycle(BRAINSTORM_CYCLE, v.brainstorm, shifted ? -1 : 1) }));
			case name === "r":
				setRows(loadRows(index, store));
				return setStatus("Reloaded from the local index.");
			case name === "x": {
				if (marks.size === 0) return setStatus("Nothing marked. d marks for delete, a for archive.");
				return setModeNow({ kind: "confirm" });
			}
		}
		const row = live.current.visible[live.current.selected];
		if (!row) return;
		const id = row.c.id;
		switch (true) {
			case name === "n":
				setTitleText(displayTitle(row.c));
				return setModeNow({ kind: "title", id });
			case name === "d" && !shifted:
				return toggleMark(id, "delete");
			case name === "a" && !shifted:
				return toggleMark(id, "archive");
			case name === "u":
				return toggleMark(id, null);
			case name === "return": {
				const s = row.verdict?.suggestion;
				if (!s) return setStatus("Not classified yet; nothing to accept.");
				setMarks((m) => {
					const next = new Map(m);
					if (s === "keep") next.delete(id);
					else next.set(id, s);
					return next;
				});
				return moveBy(1);
			}
			case name === "o":
				Bun.spawn(["open", `https://chatgpt.com/c/${id}`]);
				return setStatus("Opened in the browser.");
			case name === "c": {
				if (transcript.status !== "ready") return setStatus("Transcript not loaded yet.");
				copyToClipboard(transcript.markdown).then(
					() => setStatus(`Copied "${displayTitle(row.c)}" (${Math.round(transcript.markdown.length / 1024)} KB).`),
					(err: unknown) => setStatus(`Copy failed: ${err instanceof Error ? err.message : String(err)}`),
				);
				return;
			}
		}
	});

	const { del, arch } = markCounts();
	const topicLabel = view.topic === "all" ? "all topics" : view.topic;
	const extras = [
		view.olderThan !== "any" ? `older than ${view.olderThan}` : "",
		view.brainstorm !== "off" ? `brainstorms: ${view.brainstorm}` : "",
		view.query ? `"${view.query}"` : "",
	].filter(Boolean);
	const header = [`${visible.length} of ${rows.length}`, view.archived ? "archived" : "active", view.suggestion, topicLabel, ...extras].join(" · ");
	const marksLabel = marks.size ? `marked: ${del} delete, ${arch} archive (x to apply)` : "";

	return (
		<box flexDirection="column" width={width} height={height}>
			<box height={1} flexDirection="row" justifyContent="space-between">
				<text fg={COLOR.accent}>{`chatgpt · ${header}`}</text>
				<text fg={COLOR.accent}>{marksLabel}</text>
			</box>
			<box flexDirection="row" flexGrow={1}>
				<ListPane rows={visible} start={start} height={listHeight} width={listWidth} selected={selected} marks={marks} title="Conversations" />
				<PreviewPane row={current} transcript={transcript} store={store} scrollRef={scrollRef} />
			</box>
			<box height={1}>
				{mode.kind === "filter" ? (
					<box flexDirection="row">
						<text fg={COLOR.accent}>/</text>
						<input value={view.query} focused onInput={(q: string) => changeView(() => ({ query: q }))} placeholder="filter titles, enter to keep, esc to clear" />
					</box>
				) : (
					<text fg={COLOR.dim}>{status || "? help · / filter · 1-6 suggestion · t topic · y age · b brainstorms · n title · d/a mark · enter accept · x apply · q quit"}</text>
				)}
			</box>
			{mode.kind === "help" ? (
				<box position="absolute" top={2} left={4} width={84} height={HELP.split("\n").length + 4} border borderColor={COLOR.accent} backgroundColor="#1a1b26" title="Keys" padding={1}>
					<text fg={COLOR.fg}>{HELP}</text>
				</box>
			) : null}
			{mode.kind === "confirm" ? (
				<box position="absolute" top={3} left={6} width={70} border borderColor={COLOR.markDelete} backgroundColor="#1a1b26" title="Apply marks" padding={1} flexDirection="column">
					<text fg={COLOR.fg}>{`Archive ${arch} and permanently delete ${del} conversation(s). Deletes cannot be undone.`}</text>
					<text fg={COLOR.dim}>Type apply and press enter. Esc cancels.</text>
					<input
						focused
						value={confirmText}
						onInput={setConfirmText}
						onSubmit={() => {
							if (confirmText.trim() === "apply") void apply();
							else setStatus("Not applied: type apply exactly.");
							setConfirmText("");
							if (confirmText.trim() !== "apply") setModeNow({ kind: "browse" });
						}}
					/>
				</box>
			) : null}
			{mode.kind === "title" ? (
				<box position="absolute" top={3} left={6} width={70} border borderColor={COLOR.accent} backgroundColor="#1a1b26" title="Edit local title" padding={1} flexDirection="column">
					<text fg={COLOR.dim}>Shown only in this CLI and TUI. Enter saves; Esc cancels.</text>
					<input focused value={titleText} onInput={setTitleText} onSubmit={() => {
						try {
							const target = index.get(mode.id)[0];
							if (!target) throw new Error("Chat no longer exists locally.");
							index.setLocalTitle(target, titleText);
							const next = loadRows(index, store);
							live.current.rows = next;
							live.current.visible = visibleRows(next, live.current.view);
							setRows(next);
							setStatus("Local title saved.");
							setModeNow({ kind: "browse" });
						} catch (error) {
							setStatus(error instanceof Error ? error.message : String(error));
						}
					}} />
				</box>
			) : null}
			{mode.kind === "applying" ? (
				<box position="absolute" top={3} left={6} width={50} border borderColor={COLOR.accent} backgroundColor="#1a1b26" title="Applying" padding={1}>
					<text fg={COLOR.fg}>{`${mode.done}/${mode.total} done…`}</text>
				</box>
			) : null}
		</box>
	);
}
