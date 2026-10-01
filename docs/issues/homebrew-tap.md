# The Rust chatgpt has no Homebrew formula yet

Recorded on 2026-10-01 for stage 1 of the Rust port.

Releases publish `chatgpt-v<version>-macos-{aarch64,x86_64}.tar.gz` with `.sha256` files, and `install.sh` installs them. There is no Homebrew tap. ms-todo's release workflow has the pattern to copy: a `homebrew` job after `publish` that renders the formula from the release's checksums (`scripts/render_homebrew_formula.sh`) and pushes it to the tap with a `HOMEBREW_TAP_TOKEN` secret.

To do: create `planetaryescape/homebrew-chatgpt-cli` (authorised in D1), add the render script and its snapshot test, add the job, and have BK set the secret.
