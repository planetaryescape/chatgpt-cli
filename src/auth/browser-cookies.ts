import { existsSync, readFileSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { readChromiumCookies } from "./chromium-cookies.ts";
import { readFirefoxCookies } from "./firefox-cookies.ts";
import { readSafariCookies } from "./safari-cookies.ts";

export type BrowserCookie = { name: string; value: string; domain: string };
export type BrowserName = "dia" | "chrome" | "safari" | "firefox" | "arc" | "brave" | "edge";
export type BrowserSelection = { browser?: string; profile?: string };
export type BrowserSession = { browser: BrowserName; profile?: string; cookies: BrowserCookie[] };

const BROWSERS: BrowserName[] = ["dia", "chrome", "safari", "firefox", "arc", "brave", "edge"];
const CHROMIUM: Partial<Record<BrowserName, { directory: string; keychain: string }>> = {
	dia: { directory: "Dia/User Data", keychain: "Dia Safe Storage" },
	chrome: { directory: "Google/Chrome", keychain: "Chrome Safe Storage" },
	arc: { directory: "Arc/User Data", keychain: "Arc Safe Storage" },
	brave: { directory: "BraveSoftware/Brave-Browser", keychain: "Brave Safe Storage" },
	edge: { directory: "Microsoft Edge", keychain: "Microsoft Edge Safe Storage" },
};
const BUNDLE_IDS: Record<string, BrowserName> = {
	"company.thebrowser.dia": "dia",
	"com.google.chrome": "chrome",
	"com.apple.safari": "safari",
	"org.mozilla.firefox": "firefox",
	"company.thebrowser.browser": "arc",
	"com.brave.browser": "brave",
	"com.microsoft.edgemac": "edge",
};

export function browserForBundleId(bundleId: string): BrowserName | undefined {
	return BUNDLE_IDS[bundleId.toLowerCase()];
}

export function defaultBrowser(): BrowserName | undefined {
	if (process.platform !== "darwin") return undefined;
	const path = join(homedir(), "Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist");
	const result = Bun.spawnSync(["plutil", "-extract", "LSHandlers", "json", "-o", "-", path]);
	if (result.exitCode !== 0) return undefined;
	try {
		const handlers = JSON.parse(result.stdout.toString()) as { LSHandlerURLScheme?: string; LSHandlerRoleAll?: string; LSHandlerModificationDate?: number }[];
		const id = handlers.filter((entry) => entry.LSHandlerURLScheme === "https")
			.sort((a, b) => (b.LSHandlerModificationDate ?? 0) - (a.LSHandlerModificationDate ?? 0))[0]?.LSHandlerRoleAll;
		return id ? browserForBundleId(id) : undefined;
	} catch {
		return undefined;
	}
}

function chromiumProfiles(root: string): string[] {
	if (!existsSync(root)) return [];
	const profiles = readdirSync(root, { withFileTypes: true })
		.filter((entry) => entry.isDirectory() && (entry.name === "Default" || /^Profile \d+$/.test(entry.name)))
		.map((entry) => entry.name);
	try {
		const state = JSON.parse(readFileSync(join(root, "Local State"), "utf8")) as { profile?: { last_used?: string } };
		const lastUsed = state.profile?.last_used;
		profiles.sort((a, b) => Number(b === lastUsed) - Number(a === lastUsed) || Number(b === "Default") - Number(a === "Default") || a.localeCompare(b));
	} catch {
		profiles.sort((a, b) => Number(b === "Default") - Number(a === "Default") || a.localeCompare(b));
	}
	return profiles;
}

function firefoxProfiles(root: string): string[] {
	if (!existsSync(root)) return [];
	const profiles = readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort();
	try {
		const ini = readFileSync(join(root, "..", "profiles.ini"), "utf8");
		const preferred = ini.match(/^Default=Profiles\/(.+)$/m)?.[1]?.trim()
			?? ini.split(/^\[/m).find((section) => /^Default=1$/m.test(section))?.match(/^Path=Profiles\/(.+)$/m)?.[1]?.trim();
		profiles.sort((a, b) => Number(b === preferred) - Number(a === preferred) || a.localeCompare(b));
	} catch { /* A missing profiles.ini does not prevent scanning profile directories. */ }
	return profiles;
}

function readFromBrowser(browser: BrowserName, profile: string | undefined, home: string): BrowserSession | undefined {
	if (browser === "safari") {
		if (profile) throw new Error("Safari does not have selectable cookie profiles.");
		const cookies = readSafariCookies("chatgpt.com", home);
		return cookies.some((cookie) => cookie.name.startsWith("__Secure-next-auth.session-token")) ? { browser, cookies } : undefined;
	}
	const chromium = CHROMIUM[browser];
	const root = chromium ? join(home, "Library/Application Support", chromium.directory) : join(home, "Library/Application Support/Firefox/Profiles");
	const profiles = chromium ? chromiumProfiles(root) : firefoxProfiles(root);
	if (profile && !profiles.includes(profile)) throw new Error(`Profile "${profile}" was not found in ${browser}. Available profiles: ${profiles.join(", ") || "none"}.`);
	for (const candidate of profile ? [profile] : profiles) {
		const cookies = chromium
			? readChromiumCookies(root, candidate, chromium.keychain, "chatgpt.com")
			: readFirefoxCookies(join(root, candidate), "chatgpt.com");
		if (cookies.some((cookie) => cookie.name.startsWith("__Secure-next-auth.session-token"))) {
			return { browser, profile: candidate, cookies };
		}
	}
	return undefined;
}

export function readBrowserSession(selection: BrowserSelection = {}, home = homedir(), preferred = defaultBrowser()): BrowserSession {
	if (process.platform !== "darwin") throw new Error("Browser session reading currently supports macOS only.");
	const requested = selection.browser ?? process.env.CHATGPT_BROWSER;
	const profile = selection.profile ?? process.env.CHATGPT_BROWSER_PROFILE;
	if (profile && !requested) throw new Error("--profile requires --browser (or CHATGPT_BROWSER).");
	if (requested && !BROWSERS.includes(requested as BrowserName)) {
		throw new Error(`Unsupported browser "${requested}". Choose ${BROWSERS.join(", ")}.`);
	}
	const browser = (requested as BrowserName | undefined) ?? preferred;
	if (!browser) throw new Error(`Could not identify your macOS default browser. Choose one with --browser: ${BROWSERS.join(", ")}.`);
	const session = readFromBrowser(browser, profile, home);
	if (session) return session;
	throw new Error(`No ChatGPT session in ${browser}${profile ? ` profile "${profile}"` : ""}. Log in to chatgpt.com there and retry, or use --browser and --profile to select another session.`);
}
