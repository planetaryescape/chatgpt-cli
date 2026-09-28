import type { Conversation, MessageNode } from "../api/conversations.ts";
import type { CachedTranscript } from "../index/classification-store.ts";

export type Turn = { role: "user" | "assistant"; text: string };

// Bump when rendering changes so cached transcripts are re-rendered.
export const RENDER_VERSION = 2;
export type Canvas = { name: string; content: string };

type Message = NonNullable<MessageNode["message"]>;

// The mapping is a tree (edits and regenerations branch it). The visible thread
// is the path from current_node up to the root.
function visibleThread(convo: Conversation): Message[] {
	const messages: Message[] = [];
	let nodeId: string | null = convo.current_node;
	while (nodeId) {
		const node: MessageNode | undefined = convo.mapping[nodeId];
		if (!node) break;
		if (node.message) messages.push(node.message);
		nodeId = node.parent;
	}
	return messages.reverse();
}

// Parts mix plain strings with typed objects. Voice mode stores speech as
// audio_transcription objects; images are asset pointers we can only name.
function partText(part: unknown): string | undefined {
	if (typeof part === "string") return part;
	if (typeof part !== "object" || part === null) return undefined;
	const typed = part as { content_type?: string; text?: string };
	if (typed.content_type === "audio_transcription") return typed.text;
	if (typed.content_type === "image_asset_pointer") return "[image]";
	return undefined;
}

// Web citations and memory refs appear as private-use-character markers
// (citeturn0search1); the UI swaps each for its `alt` text.
function resolveReferences(text: string, message: Message): string {
	// Refs are in text order, so replace each one's next occurrence from a moving
	// cursor. Whitespace-only refs (the hidden sources footnote matches " ") are
	// skipped: replacing those globally deletes every space in the message.
	let out = text;
	let cursor = 0;
	for (const ref of message.metadata?.content_references ?? []) {
		if (!ref.matched_text.trim()) continue;
		const at = out.indexOf(ref.matched_text, cursor);
		if (at === -1) continue;
		const alt = ref.alt ?? "";
		out = out.slice(0, at) + alt + out.slice(at + ref.matched_text.length);
		cursor = at + alt.length;
	}
	return out.replace(/[^]*/g, "");
}

function messageText(message: Message): string {
	const parts = (message.content.parts ?? []).map(partText).filter((p): p is string => Boolean(p));
	// Image attachments already appear as [image] parts; mime_type is sometimes null.
	const files = (message.metadata?.attachments ?? [])
		.filter((a) => !a.mime_type?.startsWith("image/") && !IMAGE_FILE.test(a.name))
		.map((a) => `[attached file: ${a.name}]`);
	return resolveReferences([...parts, ...files].join("\n"), message).trim();
}

const READABLE = new Set(["text", "multimodal_text"]);
const IMAGE_FILE = /\.(png|jpe?g|gif|webp|heic)$/i;

// Image generation replies arrive as a tool message holding only the image.
function isGeneratedImage(message: Message): boolean {
	return (
		message.author.role === "tool" &&
		message.content.content_type === "multimodal_text" &&
		(message.content.parts ?? []).some((p) => typeof p === "object" && p !== null && (p as { content_type?: string }).content_type === "image_asset_pointer")
	);
}

export function visibleTurns(convo: Conversation): Turn[] {
	const turns: Turn[] = [];
	for (const message of visibleThread(convo)) {
		if (isGeneratedImage(message)) {
			turns.push({ role: "assistant", text: "[generated image]" });
			continue;
		}
		const role = message.author.role;
		if (role !== "user" && role !== "assistant") continue;
		if (message.metadata?.is_visually_hidden_from_conversation) continue;
		// Assistant messages addressed to a tool are tool calls, not replies.
		if (role === "assistant" && message.recipient && message.recipient !== "all") continue;
		if (!READABLE.has(message.content.content_type)) continue;
		const text = messageText(message);
		if (text) turns.push({ role, text });
	}
	return turns;
}

type TextdocUpdate = { pattern: string; replacement: string; multiple?: boolean };

// Canvas documents are built by tool calls: create_textdoc carries the full
// document, update_textdoc carries regex edits. Replaying them gives the final
// state, which is usually the sharpened version worth exporting.
export function finalCanvases(convo: Conversation): Canvas[] {
	const docs: Canvas[] = [];
	for (const message of visibleThread(convo)) {
		const raw = message.content.text ?? message.content.parts?.find((p) => typeof p === "string");
		if (typeof raw !== "string") continue;
		try {
			if (message.recipient === "canmore.create_textdoc") {
				const doc = JSON.parse(raw) as { name: string; content: string };
				docs.push({ name: doc.name, content: doc.content });
			} else if (message.recipient === "canmore.update_textdoc") {
				const current = docs.at(-1);
				if (!current) continue;
				for (const u of (JSON.parse(raw) as { updates: TextdocUpdate[] }).updates) {
					current.content =
						u.pattern === ".*"
							? u.replacement
							: current.content.replace(new RegExp(u.pattern, u.multiple ? "g" : ""), () => u.replacement);
				}
			}
		} catch {
			// A malformed or non-JS-compatible edit shouldn't sink the export; the
			// canvas keeps its last good state.
		}
	}
	return docs;
}

export function conversationUrl(id: string): string {
	return `https://chatgpt.com/c/${id}`;
}

export function renderTranscript(convo: Conversation): string {
	const date = new Date(convo.create_time * 1000).toISOString().slice(0, 10);
	const header = `# ${convo.title}\n\n${conversationUrl(convo.conversation_id)} · ${date} · ${convo.default_model_slug ?? "unknown model"}`;
	const turns = visibleTurns(convo).map((t) => `## ${t.role === "user" ? "Me" : "ChatGPT"}\n\n${t.text}`);
	const canvases = finalCanvases(convo).map((c) => `## Canvas (final): ${c.name}\n\n${c.content}`);
	return `${[header, ...turns, ...canvases].join("\n\n---\n\n")}\n`;
}

// The cache row for a fetched conversation; `updateTime` is the index's value,
// which is what cache lookups compare against.
export function toCachedTranscript(id: string, updateTime: string, convo: Conversation): CachedTranscript {
	const markdown = renderTranscript(convo);
	return {
		id,
		update_time: updateTime,
		render_version: RENDER_VERSION,
		markdown,
		turns: visibleTurns(convo).length,
		approx_tokens: Math.ceil(markdown.length / 4),
	};
}
