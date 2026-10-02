# Classification reference

The questions Jev answers about each chat, the rules that turn its answers into delete, archive or keep, and Luna's final review of unresolved chats.

Source of truth: `crates/daemon/src/classify/questions.json` and `deep_questions.json` (questions), `crates/daemon/src/policy/` (rules), and the versions in `Profile::builtin` (`crates/daemon/src/policy/profile.rs`). If this page and the code disagree, the code wins; update this page.

## Jev's input

For each chat Jev receives:

- `conversation`: title, created and last-updated dates, the current UTC date, number of turns, whether it's in a project, whether it's pinned.
- `content_kind`: `full transcript`, or `summary of a long conversation` for chats over ~12,000 tokens.
- `content`: the transcript as markdown, or the summary.

Summaries are written by Codex (`gpt-6-luna`), or by Claude Haiku (`claude -p`) if Codex fails. The summary prompt asks them to state whether the chat was brainstorming and whether it was quick help that could be re-asked.

## Questions

| Id | Type | What it asks |
|---|---|---|
| `worth_keeping` | Score 0–3 | How much value in finding it again: 0 nothing; 1 generic, re-askable; 2 specific to you and hard to reproduce; 3 lasting (decisions, plans, drafts, records) |
| `nothing_there` | Yes/no | Empty or trivial: a greeting, a test, an accidental start, no substantive answer |
| `re_askable` | Yes/no | Quick help you'd get again just by asking again, follow-ups and personal mentions included; also help tied to a moment that has passed, such as a trip or booking |
| `time_bound` | Yes/no | Whether the chat mainly served a specific event, deadline, trip, live situation or other finite window |
| `overtaken_by_time` | Yes/no | Whether that window has passed and this exact chat has no lasting record, artifact, thinking or work to resume |
| `unfinished` | Yes/no | Substantive work left in progress; an unanswered question doesn't count |
| `personal_record` | Yes/no | A record about your life you may need later: health, money, legal, housing, family, an important decision |
| `brainstorming` | Yes/no | Developing the user's own idea for a piece or project; routine implementation, tutorials, interview exercises and work for another organization do not count by themselves |
| `product_idea` | Yes/no | Originating, comparing or substantially shaping the user's own product or business idea; excludes generic coding, settled implementation and another organization's product |
| `brainstorm_for` | Choice | `writing` (general writing, including work with faith influences), `sermon` (writing whose primary purpose is religious teaching or reflection), `product`, `other` or `none` |
| `topic` | Choice | `employer_work`, `side_projects`, `coding_general`, `writing_creativity`, `faith`, `family_relationships`, `health`, `home_money_admin`, `career_employment`, `travel_transport`, `learning_culture`, `other` |

Yes/no answers are probabilities from 0 to 1. The score is probability-weighted, so 1.6 means "between low and moderate, leaning moderate".

`sermon` covers pieces primarily intended for religious teaching or reflection, including sermons, devotionals, Bible studies, and articles, talks or books with that main purpose. `writing` includes broader work informed by faith or containing religious references. A faith question without a piece being developed is a `faith` topic, not a brainstorm. Jev's direct `product` label requires `brainstorming` ≥ 0.6, `product_idea` ≥ 0.7, and a primary topic other than `employer_work`. Luna may confirm a borderline idea when `product_idea` is at least 0.4, but cannot turn a low product-idea score or work for an employer into the user's product brainstorming. Long-chat summaries describe a piece's main purpose, distinguish the user's product ideation from routine work, and treat faith influences as context before Jev assigns the kind.

The topic choice records one primary purpose in the generated `judgments.topic` SQLite column. Work for an employer and work on the user's own products retain their own topics even when a chat involves coding, design or writing. `writing_creativity` covers independent writing, writing craft and creative media; task-specific emails, CVs, product documents and work reports follow their subject topic. `faith` is for religious inquiry or religious teaching as the main purpose. `health` handles symptoms and treatment, while `family_relationships` handles parenting and relationship dynamics. The `other` choice is a narrow fallback.

## Rules

Rules are checked in this order. The first one that matches decides.

| # | Suggestion | When |
|---|---|---|
| 1 | keep (brainstorm) | `brainstorming` ≥ 0.6; Jev's `product` label also requires `product_idea` ≥ 0.7 and a primary topic outside `employer_work` |
| 2 | delete | `time_bound` ≥ 0.7 and `overtaken_by_time` ≥ 0.8; conflicting strong evidence of a personal record, unfinished work or lasting value makes it unsure for deeper review |
| 3 | delete | `nothing_there` ≥ 0.8, `unfinished` < 0.3, `personal_record` < 0.3, `worth_keeping` < 1 |
| 4 | delete | `re_askable` ≥ 0.7 and `personal_record` < 0.5 |
| 5 | archive | `worth_keeping` < 1.5, `unfinished` < 0.5, `personal_record` < 0.5 |
| 6 | keep | anything else |

Rule 4 ignores `unfinished`, because an unresolved quick question can still be re-asked. A past date alone never triggers rule 2. Jev must identify a finite purpose that has passed without losing a lasting record or artifact. Chats with a finite purpose that has not yet expired are re-judged when `classify` next runs at least seven UTC days later. If Jev also found strong evidence of a lasting personal record, they are rechecked in a later UTC month instead. Use `--redo` to check one sooner.

## Unsure

A suggestion is marked unsure (shown as `delete?`, `keep?` and so on) when the answers that decided it are torn. Most rules use a probability between 0.35 and 0.65, or Jev's confidence on `worth_keeping` below 0.4:

| Suggestion | Unsure when any of these is torn |
|---|---|
| keep (brainstorm) | `brainstorming` |
| delete (rule 2) | Strong conflicting evidence of a personal record, unfinished work, or lasting value |
| delete (rule 3) or archive | `unfinished`, `personal_record`, `brainstorming`, or low `worth_keeping` confidence |
| delete (rule 4) | `re_askable` or `personal_record` |
| keep (rule 6) | `re_askable`, or low `worth_keeping` confidence |

A time-bound chat with an overtaken score from 0.5 to 0.8 is also unsure. Strong but conflicting evidence for an expired chat is unsure until the deeper review resolves it.

After the regular judgment, `classify` automatically asks follow-up questions for every unsure chat in the selected set, including chats whose regular judgment was cached. It then sends any still unsure to `gpt-6-luna` for a final review. The TUI's `6` key lists those still unsure if that final review failed or lacked content.

### Follow-up questions

The follow-up uses the same cached transcript or summary and asks Jev which specific content deletion would lose:

| Id | Type | What it asks |
|---|---|---|
| `personal_record_lost` | Yes/no | A specific personal record that cannot be recovered by asking again |
| `reusable_artifact_lost` | Yes/no | A draft, plan, code, design, analysis of the user's data, or recorded decision that can be reused in its current form; tailored advice alone does not count |
| `original_thinking_lost` | Yes/no | The user's own developed argument, interpretation, or analysis; a question or preference alone does not count |
| `work_to_resume` | Yes/no | Substantive work left in progress |
| `creative_idea_lost` | Yes/no | The user's own idea, outline, or draft |
| `reaskable_without_loss` | Yes/no | Whether restating the question and relevant facts would recover materially the same value, including tailored recommendations |
| `worth_finding_again` | Score 0–3 | The value of finding this exact conversation again; a tailored but reproducible answer is low |

A strongly identified creative idea, personal record, reusable artifact, original thinking, or unfinished work becomes sure `keep`. Re-askable content becomes sure `delete` only when loss scores are very low and Jev is confident its value is low. Low-value content with no identified loss can become sure `archive`; content with substantial unique value can become sure `keep`. Conflicting or weak answers retain the original suggestion and its `?`. The follow-up can change the suggestion category as well as remove uncertainty.

Follow-up answers have their own version (`deep_questions_version`). Changing those questions re-asks only the unsure chats; changing the regular questions still re-judges the regular pass. Threshold changes in either policy file recalculate suggestions on read.

### Luna's final review

Luna reads the cached transcript or long-chat summary. It reviews chats that remain unsure after Jev's follow-up, every Jev product-brainstorm label, borderline product ideas, and possible time-expired chats with conflicting evidence. For product reviews, Jev's labels and scores are withheld to give Luna an independent reading of what happened in the conversation. A product label needs a concrete new decision made in that chat about the problem, audience, capability, workflow, value proposition, or business model. A pre-existing PRD or app concept is background; converting its specification, choosing implementation technology, explaining current functionality, or mentioning it in another task does not qualify on its own. Luna chooses keep, archive or delete, a brainstorm kind when applicable, and a short reason. For a sure Jev suggestion, the product review can remove the brainstorm label and can choose a safer keep or archive suggestion; a time-expired review can change the suggestion. For an unsure suggestion, Luna supplies the final verdict. Ambiguous content is kept; deletion covers empty chats, re-askable quick help, and one-off help whose useful moment has passed without leaving a meaningful record or artifact. Strong independent Jev evidence of both a personal record and the user's original thinking protects a chat from a Luna delete, even if the occasion has passed. Outside time-expired reviews, independent Jev evidence of a personal record or reusable artifact also prevents a Luna delete and archives the chat unless it is a brainstorm. `luna_judgments` stores these results by chat update time and `LUNA_JUDGMENT_VERSION`; changing a chat or Jev's judgment invalidates the review. A failed call leaves an unsure verdict unsure and causes `classify` to exit nonzero.

## `--check`

`archive --check` and `delete --check` act only on chats Jev backs:

- delete: suggestion is delete and not unsure
- archive: suggestion is delete or archive, and not unsure

Other matching chats are listed as held back, with their scores.

## Reading the scores

The `reason` shown in `--check` output and the TUI preview reads:

```text
nothing 0.02 · re-askable 0.93 · worth 1.1/3 · unfinished 0.26 · personal 0.30 · brainstorm 0.02
```

Each number is the answer to the question with the matching id above.

## Changing the rules

- Thresholds in `crates/daemon/src/policy/` apply every time suggestions are read. Change them and re-open the TUI or re-run `stats`; there's nothing to re-run.
- Changing a question's wording, criteria, or the set of questions needs a `questions_version` bump in `Profile::builtin`. The next `classify` then re-judges everything.
- Changing a follow-up question or criterion needs a `deep_questions_version` bump. The next `classify` re-asks the unsure chats.
- Changing Luna's final-review prompt or output meaning needs a `luna_version` bump. The next `classify` re-reviews chats still unsure after Jev, every product label, borderline product ideas and conflicting time-expired cases.
- Check a change against a handful of chats whose correct answer you know before running it on everything. A few misses are acceptable; see [Maintainer guide](../maintainers.md#locked-decisions).
