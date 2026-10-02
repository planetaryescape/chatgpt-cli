# Add chats to a project

Create a project if you need a new one:

```sh
chatgpt project create "Research Notes"
```

Then move chats from your local index into an existing project:

```sh
chatgpt sync
chatgpt project list
chatgpt project add "Garden Planner" <chat-id> --dry-run
chatgpt project add "Garden Planner" <chat-id>
```

`project create` uses ChatGPT's default project settings, rejects empty or already used names, and prints the new id and name. Add `--json` for structured output. Skip creation if the project already exists. The dry run prints the selected chat and the number that would move. The final command asks for confirmation. `--yes` skips that prompt. You can name a project by its exact name, full id, or a unique id prefix. If names match more than one project, use an id. The command rejects projects where your account lacks write access.

## Move several chats

Pass multiple ids or pipe ids from `list`:

```sh
chatgpt project add "Garden Planner" <first-chat-id> <second-chat-id> --dry-run
chatgpt list --title 'draft' --format ids | chatgpt project add "Garden Planner" - --dry-run
chatgpt list --title 'draft' --format ids | chatgpt project add "Garden Planner" -
```

`--format ids` prints one full id per line, including chats with multiline titles. `-` reads those ids from stdin. Ids can also be unique prefixes when passed as arguments. `list` uses the local index, so run `chatgpt sync` first. After ChatGPT confirms a move, the command updates the local `project_id`; a later `sync` refreshes its `update_time`. A chat already shown in the target project by the local index is skipped.

Project moves accept active chats by default, including when you pass an id or pipe ids from another command. Use `--archived` on `project add` or `project remove` to select archived chats explicitly, or `--all` to allow either state. Archived chats are outside follow-up work unless the user explicitly asks to include them.

## Remove chats from a project

Use the same id forms with `project remove`. The command only acts on selected chats currently indexed in the named project, previews them, and asks for confirmation:

```sh
chatgpt project remove "Garden Planner" <chat-id> --dry-run
chatgpt project remove "Garden Planner" <chat-id>
```

Removing a chat leaves its conversation and classification intact. The CLI clears its local project id only after ChatGPT confirms the removal. Run `chatgpt sync` afterward to refresh project metadata.

ChatGPT can change `update_time` when a chat moves. Run `chatgpt sync` afterward. It compares the moved chat's current content with the cached transcript and keeps Jev judgments, summaries, and search chunks current when only metadata changed. If the content changed, run `chatgpt classify` and `chatgpt search-index` to refresh it.

For older chats, ChatGPT may return HTTP 500 even after completing a move. The CLI reads the chat to confirm those cases. If a chat still fails, its local project id stays unchanged; run `chatgpt sync` and check it in `list --json` before retrying.

## Delete a project

```sh
chatgpt project delete "Garden Planner" --dry-run
chatgpt project delete "Garden Planner"
```

`project delete` resolves the project as `project add` does, then shows its id and how many indexed chats are in it (with the first 25). ChatGPT may delete those chats along with the project, or only take them out of it, so treat them as deleted and export any you want first. To confirm, type the project's name exactly; `--yes` skips that. The delete is sent once. After ChatGPT confirms it, the CLI clears the local project id of the chats that were in it; run `chatgpt sync` to see which ones ChatGPT kept.

## Check the result

```sh
PROJECT_ID=$(chatgpt project list --json | jq -r '.[] | select(.name == "Garden Planner") | .id')
chatgpt list --json | jq -r --arg project "$PROJECT_ID" '.[] | select(.project_id == $project) | [.id, .title] | @tsv'
```

The output shows chats whose indexed project id matches Garden Planner. For all command options, see the [CLI reference](../reference/cli.md). For the observed ChatGPT request, see [the web API notes](../explanation/chatgpt-api.md).
