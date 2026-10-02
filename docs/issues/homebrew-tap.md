# Homebrew tap: the release job can't push yet

Recorded on 2026-10-01 for stage 1 of the Rust port; updated on 2026-10-02 (stage 7).

Done:

- The tap exists: [planetaryescape/homebrew-chatgpt-cli](https://github.com/planetaryescape/homebrew-chatgpt-cli), with `Formula/chatgpt.rb` for v0.2.0 rendered from that release's checksums. `brew install planetaryescape/chatgpt-cli/chatgpt` installs it.
- `packaging/homebrew/chatgpt.rb` is the template, `scripts/render_homebrew_formula.sh` renders it from a release's `.sha256` files, and `crates/cli/tests/homebrew_formula.rs` checks the render.
- The `homebrew` job in `.github/workflows/release-please.yml` renders the formula after `publish` and pushes it to the tap with the `HOMEBREW_TAP_TOKEN` secret, as ms-todo's does.

Blocked: the repository has no `HOMEBREW_TAP_TOKEN` secret. The token the ms-todo tap uses ("ms-todo homebrew tap token" in 1Password) is a fine-grained token that can push to `planetaryescape/homebrew-ms-todo` but gets a 403 on this tap (checked with a dry-run push on 2026-10-02). Until the secret is set, the job skips with a warning and the tap stays at its last version.

To unblock (BK): add `planetaryescape/homebrew-chatgpt-cli` to that token's repository access (GitHub → Settings → Developer settings → Fine-grained tokens, Contents: read and write), then set the secret without echoing it:

```sh
op read 'op://Environment Variables/ms-todo homebrew tap token/credential' | gh secret set HOMEBREW_TAP_TOKEN -R planetaryescape/chatgpt-cli
```

Any release after that updates the tap. A release made before then needs its formula pushed by hand: `scripts/render_homebrew_formula.sh <tag> <dir with the release's .sha256 files> Formula/chatgpt.rb` in a clone of the tap.
