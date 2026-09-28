# Configure model API keys

```sh
chatgpt configure                 # show which keys are stored, never their values
chatgpt configure jev             # enter TypeSafe/Jev API key without terminal echo
chatgpt configure openai          # enter OpenAI API key
chatgpt configure anthropic       # enter Anthropic API key
chatgpt configure openai --remove # remove a stored key
```

The file is `${XDG_CONFIG_HOME:-~/.config}/chatgpt-cli/config.json` with owner-only permissions (`0600`). A non-interactive command can provide one key on stdin. Do not put keys in command arguments or shell history.

For each provider, the matching environment variable (`TYPESAFE_API_KEY`, `OPENAI_API_KEY`, or `ANTHROPIC_API_KEY`) takes precedence over the config file. If an OpenAI key is configured, Luna review and titles call the OpenAI API instead of `codex`; long-chat summaries use the OpenAI API instead of `codex`. If an Anthropic key is configured, the summary fallback calls the Anthropic API instead of `claude`. Without OpenAI or Anthropic keys, the subscription CLIs remain available. API usage may incur charges.
