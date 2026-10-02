# Saved-memory classification

Saved memories are facts ChatGPT can carry into future conversations. They have a different purpose from chat history: a finished chat may be worth retaining as an archive while its temporary saved-memory instruction is no longer useful. Classification never acts on source chats and never deletes a memory automatically.

## Rubric

1. **Keep durable context.** Identity, beliefs, language, relationships, enduring preferences, meaningful personal history, and active long-term work are worth remembering. A detail can be sensitive and still be valuable when the user uses it for support. Sensitivity alone is not a delete criterion.
2. **Suggest delete for clear loss of current value.** Examples are completed one-off tasks, explicitly expired routines or deadlines, narrow implementation facts from past tasks, operational claims clearly replaced by newer facts, or a full duplicate whose useful information another saved entry contains. A project need not be abandoned for an old, narrow task detail to stop helping future answers.
3. **Review uncertain current facts.** Do not infer that a project was abandoned from age, an archived source chat, or a newer unrelated conversation. A past event may still encode a lasting preference, achievement, or original idea. Similar subject matter does not make two memories duplicates. Review only when whether the fact is still true materially changes its usefulness.
4. **Prefer precision over clearing the list.** If evidence is mixed, keep or review. The goal is a smaller set of high-confidence delete suggestions, not a forced answer for every entry.

## Two stages

Jev scores lasting value, expiry, supersession, and full redundancy, using the memory and up to three related saved entries. Only clear enduring context bypasses deeper review. Every other result goes to GPT-6 Luna in small batches; Luna can decide keep, delete, or review and gives a short reason. A quick-stage score alone can never produce a delete suggestion.

When Jev sees strong lasting value without expiry or duplication but Luna suggests delete, the final result stays review. This disagreement guard protects meaningful personal context from a single model's narrow reading.

The SQLite cache keys each result to the memory text and timestamp, related-entry text, rubric version (`memory_version` in `Profile::builtin`), and current UTC month. Bump the version when changing questions, thresholds, or Luna instructions. A new month rechecks temporary facts that may have expired since the prior run.
