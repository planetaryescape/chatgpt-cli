# Negated shorthands inside a character class keep Unicode meaning

**Resolved in stage 7.** `crates/daemon/src/js_regex.rs` parses every class member as JS (Annex B) reads it: `[\W]`, `[\D]` and `[\S]` become nested negated ASCII (or JS-space) classes, `[\b]` is a backspace and `[\B]` a `B`. It also matches by UTF-16 code unit (see the `js.rs:125` entry in `cubic-pr3-followups.md`). `regress` was evaluated and not adopted: it backtracks without a budget (`^(a+)+$` on 26 `a`s and a `!` took 1.4 s, doubling per `a`), and these patterns run inside the daemon. Tests: `js_regex::tests`, each checked against node.

Recorded on 2026-10-02 from the stage 1 review (`crates/daemon/src/js.rs`, `regex_source`).

`js::regex` translates JS regex syntax for `--title` and canvas edits. Outside a character class it maps `\W`, `\D`, `\S` and `\B` to their JS (ASCII or JS-whitespace) forms. Inside a class it leaves `\W`, `\D` and `\S` as they are, so Rust's Unicode meaning applies: `[\W]` doesn't match `é` in Rust but does in JS (where `\w` is ASCII-only, so `é` is a non-word character).

Impact: rare. A canvas edit whose pattern uses one of these inside a class, applied to non-ASCII text, can render differently from the TS CLI. The cache reconcile compares renders, so a mismatch can leave a cache stale or, in the unlucky case where the different render happens to match, keep a changed transcript's caches current.

Fix: inside a class, expand a negated shorthand into the complement of its ASCII set, which needs a class-subtraction or the `regress` crate (ECMAScript semantics) instead of the translator.
