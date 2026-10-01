# Third-party code

Code copied in and patched. Not `vendor/`: common global gitignores exclude that name, which silently left it out of a commit once.

## impit

`third_party/impit` is the `impit` crate from https://github.com/apify/impit at
`152e7db1d39b35affd9635675b575e8ba5d0565f` (tag `js-0.14.5`), wired in with
`[patch."https://github.com/apify/impit"]` in the workspace `Cargo.toml`.
Licence: `LICENSE.md` (Apache-2.0), copied from the upstream repository root.

One change, in `src/impit.rs` (search for `chatgpt-cli patch`): `Impit::new`
writes `IMPIT_H2_PSEUDOHEADERS_ORDER` only when the variable differs from the
fingerprint's order. Upstream writes it on every build. Writing the environment
while other threads read it is unsound (`docs/issues/impit-set-var-race.md`).
The daemon makes sure the variable already holds Chrome's order before its
runtime starts, and `chatgpt::http` refuses to build a client otherwise, so no
client build writes the environment.

To update: copy the `impit/` directory of the new upstream rev here, reapply
the change, and update the rev above and in the workspace manifest.
