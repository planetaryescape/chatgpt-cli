// Progress and timing on stderr. A live line on a terminal; plain lines
// otherwise, so piped or logged runs stay readable. Hand-rolled because it has
// to interleave with other stderr notices (rate-limit waits) and report timings.

const isTTY = Boolean(process.stderr.isTTY);

// Status on stderr in the terminal's normal colour. Bun prints console.error in
// red, which made successful runs look like failures; that stays for real errors.
export function note(message: string) {
	process.stderr.write(`${message}\n`);
}

export function formatDuration(ms: number): string {
	const s = Math.round(ms / 1000);
	if (s < 60) return ms < 10_000 ? `${(ms / 1000).toFixed(1)}s` : `${s}s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m}m${String(s % 60).padStart(2, "0")}s`;
	return `${Math.floor(m / 60)}h${String(m % 60).padStart(2, "0")}m`;
}

function bar(done: number, total: number, width = 20): string {
	const filled = total ? Math.round((done / total) * width) : 0;
	return `${"█".repeat(filled)}${"░".repeat(width - filled)}`;
}

export class Step {
	private readonly startedAt = Date.now();
	private lastPlainLog = 0;

	constructor(
		private readonly label: string,
		private readonly total?: number,
	) {
		if (!isTTY) note(`${label}…`);
		else this.update(0);
	}

	update(done: number, detail = "") {
		const elapsed = Date.now() - this.startedAt;
		let line = `${this.label}`;
		if (this.total !== undefined) {
			const rate = elapsed > 0 ? done / (elapsed / 1000) : 0;
			const eta = rate > 0 && done < this.total ? ` · ${formatDuration(((this.total - done) / rate) * 1000)} left` : "";
			line += `  ${bar(done, this.total)} ${done}/${this.total} · ${rate.toFixed(1)}/s${eta}`;
		} else {
			line += `  ${done} so far`;
		}
		line += ` · ${formatDuration(elapsed)}${detail ? ` · ${detail}` : ""}`;
		if (isTTY) {
			process.stderr.write(`\r\x1b[2K${line}`);
		} else if (Date.now() - this.lastPlainLog > 5_000) {
			// Without a live line, report at most every 5s.
			this.lastPlainLog = Date.now();
			note(line);
		}
	}

	finish(summary: string) {
		const line = `${summary} (${formatDuration(Date.now() - this.startedAt)})`;
		if (isTTY) process.stderr.write(`\r\x1b[2K${line}\n`);
		else note(line);
	}
}

export function timer() {
	const startedAt = Date.now();
	return () => formatDuration(Date.now() - startedAt);
}
