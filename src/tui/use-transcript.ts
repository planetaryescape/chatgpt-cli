import { useEffect, useState } from "react";
import type { ChatGPTClient } from "../api/client.ts";
import { getConversationsBatch } from "../api/conversations.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import type { IndexedConversation } from "../index/store.ts";
import { RENDER_VERSION, toCachedTranscript } from "../render/transcript.ts";

export type TranscriptState = { status: "idle" | "loading" | "ready" | "error"; markdown: string; error?: string };

// Waiting before a fetch keeps fast j/k scrolling from firing a request per row.
const DEBOUNCE_MS = 200;

export function useTranscript(client: ChatGPTClient, store: ClassificationStore, c: IndexedConversation | undefined): TranscriptState {
	const [state, setState] = useState<TranscriptState>({ status: "idle", markdown: "" });

	useEffect(() => {
		if (!c) {
			setState({ status: "idle", markdown: "" });
			return;
		}
		const cached = store.transcript(c.id, c.update_time, RENDER_VERSION);
		if (cached) {
			setState({ status: "ready", markdown: cached.markdown });
			return;
		}
		setState({ status: "loading", markdown: "" });
		let cancelled = false;
		const timer = setTimeout(async () => {
			try {
				const [convo] = await getConversationsBatch(client, [c.id]);
				if (!convo) throw new Error("ChatGPT didn't return this conversation (deleted?)");
				const transcript = toCachedTranscript(c.id, c.update_time, convo);
				store.saveTranscript(transcript);
				if (!cancelled) setState({ status: "ready", markdown: transcript.markdown });
			} catch (err) {
				if (!cancelled) setState({ status: "error", markdown: "", error: err instanceof Error ? err.message : String(err) });
			}
		}, DEBOUNCE_MS);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	}, [client, store, c]);

	return state;
}
