import type { Suggestion } from "../classify/policy.ts";

// Tokyo Night-ish palette: readable on dark terminals, distinct per suggestion.
export const COLOR = {
	fg: "#c0caf5",
	dim: "#565f89",
	accent: "#7aa2f7",
	border: "#3b4261",
	focusBorder: "#7aa2f7",
	selectedBg: "#283457",
	markDelete: "#f7768e",
	markArchive: "#e0af68",
} as const;

export const SUGGESTION_COLOR: Record<Suggestion, string> = {
	delete: "#f7768e",
	archive: "#e0af68",
	keep: "#9ece6a",
};
