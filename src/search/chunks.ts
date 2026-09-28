// Bump when the text sent to FTS or the embedding model changes.
export const CHUNK_VERSION = 1;
export const MAX_CHUNK_CHARS = 800;
const OVERLAP_CHARS = 80;

export function transcriptChunks(markdown: string): string[] {
	const messages = markdown.split(/\n---\n\n/).slice(1);
	const sections = messages.length ? messages : [markdown];
	const chunks: string[] = [];
	for (const section of sections) {
		const text = section.trim();
		if (!text) continue;
		for (let start = 0; start < text.length;) {
			let end = Math.min(start + MAX_CHUNK_CHARS, text.length);
			if (end < text.length) {
				const boundary = Math.max(text.lastIndexOf("\n", end), text.lastIndexOf(" ", end));
				if (boundary > start + MAX_CHUNK_CHARS / 2) end = boundary;
			}
			const chunk = text.slice(start, end).trim();
			if (chunk) chunks.push(chunk);
			if (end === text.length) break;
			start = Math.max(start + 1, end - OVERLAP_CHARS);
		}
	}
	return chunks.length ? chunks : [markdown.trim()];
}
