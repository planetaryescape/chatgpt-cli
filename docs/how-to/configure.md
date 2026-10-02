# Configure browser and model API keys

## Choose a ChatGPT browser

`chatgpt` uses the macOS default browser unless you select another one. Put global flags before the command:

```sh
chatgpt --browser chrome sync
chatgpt --browser chrome --profile "Profile 1" sync
```

Supported browser names are `safari`, `chrome`, `firefox`, `dia`, `arc`, `brave` and `edge`. `--profile` takes a Chromium or Firefox profile directory name. To keep the choice for your shell, set `CHATGPT_BROWSER` and optionally `CHATGPT_BROWSER_PROFILE`; no session token is saved in the CLI config. An index holds one ChatGPT account's chats, and a sync from another account is refused. If you use different accounts, give each one its own instance with `CHATGPT_INSTANCE=<name>`, which has its own daemon and index.

## Model API keys

```sh
chatgpt configure                 # show which keys are stored, never their values
chatgpt configure jev             # enter TypeSafe/Jev API key without terminal echo
chatgpt configure openai          # enter OpenAI API key
chatgpt configure anthropic       # enter Anthropic API key
chatgpt configure openai --remove # remove a stored key
```

The file is `${XDG_CONFIG_HOME:-~/.config}/chatgpt-cli/config.json` with owner-only permissions (`0600`). A non-interactive command can provide one key on stdin. Do not put keys in command arguments or shell history.

For each provider, the matching environment variable (`TYPESAFE_API_KEY`, `OPENAI_API_KEY`, or `ANTHROPIC_API_KEY`) takes precedence over the config file. If an OpenAI key is configured, Luna review and titles call the OpenAI API instead of `codex`; long-chat summaries use the OpenAI API instead of `codex`. If an Anthropic key is configured, the summary fallback calls the Anthropic API instead of `claude`. Without OpenAI or Anthropic keys, the subscription CLIs remain available. API usage may incur charges.

## Jev on new chats in the background

With a Jev key stored by `chatgpt configure jev`, the background daemon judges new and changed chats with Jev after each sync (at most 50 at a time), so `list --suggest` stays current between `classify` runs. It runs only Jev's first pass, never its follow-up, Luna or a summary, and never for a key that's only in your environment. `chatgpt daemon status` shows what it judged and spent today. To switch it off, add `"auto_jev": false` to the config file; `chatgpt configure` keeps the setting.

## The background daemon

Every command but `configure` talks to a background daemon, and the first one starts it. It holds your session, keeps the index, search index and embeddings fresh, and runs classification. Nothing needs configuring; these commands manage it:

```sh
chatgpt daemon status      # what it's doing: last and next sync, search index, embeddings, background Jev
chatgpt daemon logs -f     # follow its log (-n for more lines)
chatgpt daemon stop        # stop it; the next command starts it again
chatgpt daemon install     # start it at login (writes a LaunchAgent; prints how to load it)
chatgpt daemon uninstall   # stop starting it at login
```

A command started with `--browser` or `--profile` passes that choice to the daemon. After you upgrade `chatgpt`, the next command restarts an older daemon by itself.
