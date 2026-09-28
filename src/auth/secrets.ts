import { readConfig } from "./config.ts";

export type SecretName = "TYPESAFE_API_KEY" | "OPENAI_API_KEY" | "ANTHROPIC_API_KEY";

const CONFIG_NAMES = { TYPESAFE_API_KEY: "jev", OPENAI_API_KEY: "openai", ANTHROPIC_API_KEY: "anthropic" } as const;

export function findSecret(name: SecretName): string | null {
	return process.env[name]?.trim() || readConfig()[CONFIG_NAMES[name]] || null;
}

export function requireSecret(name: SecretName): string {
	const value = findSecret(name);
	if (!value) throw new Error(`${name} is not configured. Run \`chatgpt configure ${CONFIG_NAMES[name]}\` or set ${name}.`);
	return value;
}
