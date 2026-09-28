import { createCliRenderer } from "@opentui/core";
import { createRoot } from "@opentui/react";
import type { ChatGPTClient } from "../api/client.ts";
import type { ClassificationStore } from "../index/classification-store.ts";
import type { ConversationIndex } from "../index/store.ts";
import { App } from "./app.tsx";

export async function runTui(client: ChatGPTClient, index: ConversationIndex, store: ClassificationStore) {
	// Ctrl-C is handled in the app so the renderer can restore the terminal first.
	const renderer = await createCliRenderer({ exitOnCtrlC: false });
	const done = new Promise<void>((resolve) => renderer.once("destroy", () => resolve()));
	createRoot(renderer).render(<App client={client} index={index} store={store} />);
	await done;
}
