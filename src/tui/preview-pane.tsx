import type { ScrollBoxRenderable } from "@opentui/core";
import type { RefObject } from "react";
import { SUMMARY_PROMPT_VERSION } from "../classify/summarise.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import { displayTitle } from "../index/store.ts";
import type { Row } from "./model.ts";
import { COLOR, SUGGESTION_COLOR } from "./theme.ts";
import type { TranscriptState } from "./use-transcript.ts";

type Props = {
	row: Row | undefined;
	transcript: TranscriptState;
	store: ClassificationStore;
	scrollRef: RefObject<ScrollBoxRenderable | null>;
};

export function PreviewPane({ row, transcript, store, scrollRef }: Props) {
	if (!row) {
		return (
			<box border borderColor={COLOR.border} flexGrow={1}>
				<text fg={COLOR.dim}>Nothing selected.</text>
			</box>
		);
	}
	const { c, verdict, topic, judgment } = row;
	// Long chats were judged from a summary; it's the best overview of them.
	const summary = judgment?.content_kind === "summary" ? store.summary(c.id, c.update_time, SUMMARY_PROMPT_VERSION)?.summary : undefined;
	const meta = [
		`updated ${c.update_time.slice(0, 10)}`,
		`created ${c.create_time.slice(0, 10)}`,
		c.project_id ? "in a project" : "",
		c.pinned ? "pinned" : "",
		c.is_archived ? "archived" : "",
	].filter(Boolean);

	return (
		<box border borderColor={COLOR.border} title={displayTitle(c)} flexGrow={1} flexDirection="column">
			<text fg={COLOR.dim} flexShrink={0}>{meta.join(" · ")}</text>
			{verdict ? (
				<text flexShrink={0}>
					<span fg={COLOR.dim}>{verdict.luna ? "Luna: " : "Jev: "}</span>
					<span fg={SUGGESTION_COLOR[verdict.suggestion]}>
						<b>{verdict.suggestion.toUpperCase()}</b>
					</span>
					<span fg={COLOR.dim}>{`${verdict.unsure ? " (unsure)" : ""} · ${topic ?? ""} · ${verdict.reason}`}</span>
				</text>
			) : (
				<text fg={COLOR.dim} flexShrink={0}>Jev: not classified yet (run `chatgpt classify`)</text>
			)}
			<scrollbox ref={scrollRef} flexGrow={1} marginTop={1}>
				{summary ? <text fg={COLOR.accent}>{`Summary\n${summary}\n\n${"─".repeat(40)}\n`}</text> : null}
				{transcript.status === "loading" ? <text fg={COLOR.dim}>Loading transcript…</text> : null}
				{transcript.status === "error" ? <text fg={SUGGESTION_COLOR.delete}>{`Couldn't load: ${transcript.error}`}</text> : null}
				{transcript.status === "ready" ? <text fg={COLOR.fg}>{transcript.markdown}</text> : null}
			</scrollbox>
		</box>
	);
}
