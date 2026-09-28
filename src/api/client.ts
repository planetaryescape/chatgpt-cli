import { type HttpMethod, Impit } from "impit";
import { readBrowserSession, type BrowserSelection } from "../auth/browser-cookies.ts";

const BASE = "https://chatgpt.com";
const MAX_RETRIES = 4;

export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly path: string,
		readonly body: string,
	) {
		super(`${status} from ${path}: ${body.slice(0, 300)}`);
	}
}

// Cloudflare challenges any client whose TLS/HTTP2 fingerprint isn't a real
// browser's (plain fetch and curl both get 403). impit impersonates Chrome's.
// Even then roughly 1 in 7 fresh connections gets a challenge; a new connection
// usually passes, so challenges are retried on a fresh client.
export class ChatGPTClient {
	private http = new Impit({ browser: "chrome" });
	private cookieHeader: string | undefined;
	private sessionSource: string | undefined;
	private accessToken: string | undefined;

	constructor(private readonly browserSelection: () => BrowserSelection = () => ({})) {}

	private cookies(): string {
		if (!this.cookieHeader) {
			const session = readBrowserSession(this.browserSelection());
			this.sessionSource = `${session.browser}${session.profile ? ` profile "${session.profile}"` : ""}`;
			this.cookieHeader = session.cookies.map((c) => `${c.name}=${c.value}`).join("; ");
		}
		return this.cookieHeader;
	}

	private async send(method: HttpMethod, path: string, headers: Record<string, string>, body?: string): Promise<string> {
		for (let attempt = 0; ; attempt++) {
			const res = await this.http.fetch(`${BASE}${path}`, { method, headers: { cookie: this.cookies(), ...headers }, body });
			const text = await res.text();
			if (res.status >= 200 && res.status < 300) return text;
			const challenged = res.status === 403 && res.headers.get("cf-mitigated") === "challenge";
			// 500 is an application error (e.g. legacy-chat rename) and repeats; only
			// gateway errors are transient.
			const retryable = challenged || res.status === 429 || [502, 503, 504].includes(res.status);
			if (!retryable || attempt >= MAX_RETRIES) {
				if (challenged) throw new Error(`Cloudflare kept challenging requests to ${path}. Wait a minute and retry.`);
				throw new ApiError(res.status, path, text);
			}
			if (challenged) this.http = new Impit({ browser: "chrome" });
			// 429s carry no retry-after and last longer than a gateway blip.
			const retryAfter = Number(res.headers.get("retry-after"));
			const backoffMs = (res.status === 429 ? 5000 : 1000) * 2 ** attempt;
			if (res.status === 429) process.stderr.write(`\nrate limited by ChatGPT; waiting ${backoffMs / 1000}s\n`);
			await Bun.sleep(Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter * 1000 : backoffMs);
		}
	}

	private async token(): Promise<string> {
		if (this.accessToken) return this.accessToken;
		const session = JSON.parse(await this.send("GET", "/api/auth/session", {})) as { accessToken?: string };
		if (!session.accessToken) {
			throw new Error(`ChatGPT session in ${this.sessionSource} has expired. Open chatgpt.com there to refresh it, then retry.`);
		}
		this.accessToken = session.accessToken;
		return this.accessToken;
	}

	async request<T>(method: HttpMethod, path: string, body?: unknown): Promise<T> {
		const headers: Record<string, string> = { authorization: `Bearer ${await this.token()}`, accept: "application/json" };
		if (body !== undefined) headers["content-type"] = "application/json";
		const text = await this.send(method, path, headers, body === undefined ? undefined : JSON.stringify(body));
		return (text ? JSON.parse(text) : null) as T;
	}
}
