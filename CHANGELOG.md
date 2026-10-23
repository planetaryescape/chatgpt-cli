# Changelog

## [0.2.7](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.6...v0.2.7) (2026-10-23)


### Features

* support Chromium authentication and Linux x86_64 releases ([6d198cf](https://github.com/planetaryescape/chatgpt-cli/commit/6d198cf31e30d28c1fee067589fa106cb2eceee2))

## [0.2.6](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.5...v0.2.6) (2026-10-22)


### Features

* tui pane focus and $PAGER view ([e6867f5](https://github.com/planetaryescape/chatgpt-cli/commit/e6867f5479d3f894954f8de9800d1a2c4d6d03bc))


### Bug Fixes

* Ctrl-C in the pager is the pager's alone ([079ef14](https://github.com/planetaryescape/chatgpt-cli/commit/079ef14b4433c3f8820e3e2034cfee42dd15a3bf))
* the pager never gets a chat's escape sequences ([b96df1e](https://github.com/planetaryescape/chatgpt-cli/commit/b96df1e9c336439ee2354b985fc4efbfe32b0665))


### Documentation

* count the codex fallback among the model-path commands ([e8aa3ef](https://github.com/planetaryescape/chatgpt-cli/commit/e8aa3ef9f4c3a2b466d07703b9874edcad8d11d4))

## [0.2.5](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.4...v0.2.5) (2026-10-22)


### Bug Fixes

* classifying a long chat must not pay for the search index ([de9e194](https://github.com/planetaryescape/chatgpt-cli/commit/de9e194e8e0dc4fa0d0e5f45fdc701d91e606137))

## [0.2.4](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.3...v0.2.4) (2026-10-21)


### Bug Fixes

* the review reads a whole escape sequence, and the backlog says what is true ([31bd379](https://github.com/planetaryescape/chatgpt-cli/commit/31bd37930ea8479e2691da29673b469019258fd6))

## [0.2.3](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.2...v0.2.3) (2026-10-21)


### Bug Fixes

* the TUI keeps the chats that failed, and names every failure ([dd9e95b](https://github.com/planetaryescape/chatgpt-cli/commit/dd9e95be93f0583900d93f9b2e036606e7fd7605))

## [0.2.2](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.1...v0.2.2) (2026-10-21)


### Bug Fixes

* the daemon's status counts the index, not the passes' counters ([02d6ae6](https://github.com/planetaryescape/chatgpt-cli/commit/02d6ae682a4664a110e8de8328fff8eb6e73315e))

## [0.2.1](https://github.com/planetaryescape/chatgpt-cli/compare/v0.2.0...v0.2.1) (2026-10-02)


### Features

* project delete, Homebrew formula, and TUI/review gap fixes ([#21](https://github.com/planetaryescape/chatgpt-cli/issues/21)) ([54bb05e](https://github.com/planetaryescape/chatgpt-cli/commit/54bb05e372f21ef5a19de65ccca140500a21be2c))


### Bug Fixes

* close the search, JS-compatibility and CLI parsing gaps ([#20](https://github.com/planetaryescape/chatgpt-cli/issues/20)) ([360615f](https://github.com/planetaryescape/chatgpt-cli/commit/360615f1c82bbef1cf54e1b7fd997acd032d908a))
* close the sync and daemon robustness gaps ([#18](https://github.com/planetaryescape/chatgpt-cli/issues/18)) ([cb797a8](https://github.com/planetaryescape/chatgpt-cli/commit/cb797a87269ededfe26a68b9bd13fae026f9217e))

## [0.2.0](https://github.com/planetaryescape/chatgpt-cli/compare/v0.1.5...v0.2.0) (2026-10-02)


### ⚠ BREAKING CHANGES

* Rust-only CLI; remove the TS CLI, the bridge and the TS import ([#16](https://github.com/planetaryescape/chatgpt-cli/issues/16))

### Features

* Rust-only CLI; remove the TS CLI, the bridge and the TS import ([#16](https://github.com/planetaryescape/chatgpt-cli/issues/16)) ([4663abc](https://github.com/planetaryescape/chatgpt-cli/commit/4663abc0e1ed696938e1d3a7cb6c53359c1997c7))

## [0.1.5](https://github.com/planetaryescape/chatgpt-cli/compare/v0.1.4...v0.1.5) (2026-10-02)


### Features

* native TUI, review, and parted answers for any size ([#14](https://github.com/planetaryescape/chatgpt-cli/issues/14)) ([45d9668](https://github.com/planetaryescape/chatgpt-cli/commit/45d9668789478499b1bd558be0055dbfd24d8d1f))

## [0.1.4](https://github.com/planetaryescape/chatgpt-cli/compare/v0.1.3...v0.1.4) (2026-10-02)


### Features

* native classify, titles, memory classify, configure and background Jev ([#12](https://github.com/planetaryescape/chatgpt-cli/issues/12)) ([8c073a6](https://github.com/planetaryescape/chatgpt-cli/commit/8c073a6073d6ffc631625a25940482ba5e05a92b))

## [0.1.3](https://github.com/planetaryescape/chatgpt-cli/compare/v0.1.2...v0.1.3) (2026-10-02)


### Features

* native archive, delete, rename, projects, memories and the Jev guard ([#10](https://github.com/planetaryescape/chatgpt-cli/issues/10)) ([55d8eb1](https://github.com/planetaryescape/chatgpt-cli/commit/55d8eb124a90bf356c3f891c66afb1ac3904349c))

## [0.1.2](https://github.com/planetaryescape/chatgpt-cli/compare/v0.1.1...v0.1.2) (2026-10-02)


### Features

* native semantic, hybrid and remote search with background embeddings ([#8](https://github.com/planetaryescape/chatgpt-cli/issues/8)) ([d610075](https://github.com/planetaryescape/chatgpt-cli/commit/d6100757b882ef0446f3cc5ba337474b0559d78f))

## [0.1.1](https://github.com/planetaryescape/chatgpt-cli/compare/v0.1.0...v0.1.1) (2026-10-02)


### Features

* native export and lexical search with a background indexer ([#5](https://github.com/planetaryescape/chatgpt-cli/issues/5)) ([e2ab3c2](https://github.com/planetaryescape/chatgpt-cli/commit/e2ab3c2e152948f738fb0876341d2f8d278c6275))


### Bug Fixes

* bump only workspace crates in Cargo.lock on release ([#7](https://github.com/planetaryescape/chatgpt-cli/issues/7)) ([cf5fb1f](https://github.com/planetaryescape/chatgpt-cli/commit/cf5fb1fe2ac35480c5ae301f8b775ab28d6cb855))

## 0.1.0 (2026-10-01)


### Features

* Rust chatgpt CLI with a sync daemon ([#3](https://github.com/planetaryescape/chatgpt-cli/issues/3)) ([7c5c02c](https://github.com/planetaryescape/chatgpt-cli/commit/7c5c02c26803b8ba97e21420c742ff1a41f00bb1))
