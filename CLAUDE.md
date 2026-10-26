# XmlCraft — instructions for agents

XmlCraft is a clean-room, open-source, Rust-native XML editor aiming at Altova XMLSpy parity for the features that matter most: a professional XML editor, XSD and DTD support, validation, and schema and sample generation. It runs natively on macOS, Windows and Linux. It is a sibling of `../pdfcraft` and follows the same conventions, so when this file is silent, do what PdfCraft does.

## Start every session here
1. Read `plan/STATUS.md`: the current milestone, the next unchecked task and any blockers. `ROADMAP.md` is the one-page summary (stage, numbers, progress log) and `docs/roadmap.md` the milestone table; at the end of every session update the milestone row and add a line to the ROADMAP.md progress log.
   - Read `docs/gaps.md` (the ranked gaps) and `docs/target-app-parity.md` (the numbers and how they are measured) before choosing work. Prefer the gaps over new features, and keep both documents true when things change. Progress docs open with a status line (last reviewed, last updated, change, target) and end with a revision history.
   - "Shipped" in `parity/` means "exists and tested", not "as good as XMLSpy". Don't mark a feature shipped on a generic test, and say in its notes what is still missing.
2. Read that task in `plan/execution-plan.md` §7, the relevant section of `docs/architecture.md`, and the README of the crate you're touching.
3. Work in this loop: orient → plan → implement + test → verify → record → commit. Don't stop to ask unless the decision is the user's (see "Decisions that are the user's" below).

`plan/` is gitignored (local-only, like PdfCraft). The machine-readable parity checklist lives in `parity/` (committed).

## Non-negotiables
- **Assets: read `AGENTS.md` §1 before adding or showing any icon, image, font, schema or document.** No assets from Altova products, ever. Only openly licensed or contributor-original assets are allowed, each with an entry in `ATTRIBUTION.toml`. `cargo xtask assets` enforces this. `AGENTS.md` overrides this file.
- **Bundled schemas are unmodified upstream files** under a closed licence allowlist (`AGENTS.md` §1.6). Adding a set needs the owner's approval.
- **Clean-room.**
  - Never read, disassemble or copy anything inside an XMLSpy installation (names and listings only).
  - Behaviour comes from public specs (W3C XML, Namespaces, XML Schema 1.0, XPath; OASIS XML Catalogs), Altova's public documentation and black-box observation (`plan/xmlspy/`, gitignored).
  - Never copy GPL/AGPL code. libxml2/`xmllint`, Xerces, Saxon and the Rust XSD crates (`xsd-schema`, `uppsala`, `anyxml`, `xmloxide`) are black-box test oracles only. Never read their source to learn how to implement something.
- **Privacy.** When observing XMLSpy, use synthetic fixtures only. Never capture licence keys, account information or recent files. Capture by window id. Never commit XMLSpy outputs, corpus files or personal data.
- **Network.** Only grammar resources and an on-request update check may touch the network, and only as `AGENTS.md` §4 describes. No telemetry. Any new outbound access is the owner's decision.
- **Layering.** Nothing below L7 depends on egui/winit/eframe/rfd/craft-ui (`docs/architecture.md`, enforced by `cargo xtask layers`). Every crate in L0–L7 must also compile for `wasm32-unknown-unknown` (`cargo xtask wasm`); native-only code (files, network, threads) sits behind `cfg(not(target_arch = "wasm32"))` in `platform`.
- **The text is the model.** (This replaces PdfCraft's "Fidelity" rule.)
  - The document's bytes are the source of truth, and everything else (the syntax tree, diagnostics, outline) is derived from them.
  - Opening a file and saving it without edits writes **identical bytes**: encoding, BOM, XML declaration, mixed line endings and trailing whitespace all survive.
  - Never change bytes the user did not edit. The formatter, quick fixes, Assign Schema and the generators all emit **minimal edits** into the buffer, never a rewritten document. New lines use the document's dominant line ending.
  - If a character cannot be represented in the file's encoding, saving stops and asks: write a character reference, or change the encoding. Never substitute `?`.
  - Saves are atomic (`craft-atomic`). If the file changed on disk, offer a reload; never overwrite silently.
  - Never silently repair or drop data. Where the editor is lenient (it must keep working on broken XML), record what it did.
- **Never panic.** Every XML file, schema, catalog, downloaded resource, script, MCP call, settings file and keystroke is untrusted input, and none of it may crash the app or lose the user's work. This outranks feature work. Return `Result` (the crate's error enum) and propagate with `?`, or fall back leniently and record it. In non-test code:
  - No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!` unless it is provably infallible, with a comment saying why.
  - No indexing or slicing with input-derived positions (use `get`, and slice strings only at char boundaries).
  - Use checked or saturating arithmetic on input-derived numbers, and guard against division by zero and NaN/inf casts.
  - Cap allocations sized by input, and bound recursion and entity expansion with depth limits or seen-sets.
  - Handle lock poisoning.
  - No `unsafe` (`unsafe_code = "forbid"`).
  - Every crash fix gets a synthetic regression test. See `AGENTS.md` §5.
- **Rust only** in the product and build (`xtask`). No handwritten JS/TS.
- **Time goes through `chrono`** (workspace dependency). Never hand-roll UTC/calendar conversion (days-to-date algorithms, leap-year math); a crate owns only its output format string. The one exception is range extension: past the years chrono can represent (XSD allows years beyond that), shift the date by whole 400-year Gregorian cycles (146,097 days, after which the calendar repeats), let chrono compute the month and day, and add the cycles back to the year (splitting seconds into hours, minutes and seconds is plain arithmetic).
- **Review every update as you make it, at four zooms.** Not an end-of-task pass — a per-update habit, so a defect never gets built on. **Clause:** checked arithmetic, `get()` not `[i]`, char-boundary slicing, the right error through `?`, `<` against `<=`, byte offsets against char offsets against UTF-16 columns, the 1-based/0-based seam, capped allocations. **Function:** is it total — name an input that panics; walk empty, zero, max, malformed, cyclic; is the return type honest. **File/module:** two sources of truth, dead or duplicated code, is the public surface still smallest and the doc comment still true. **Project:** layering, wasm, undo/redo, save fidelity, encodings, a matching `automation` tool or registry entry, and whether a `parity/` status or note just became untrue. Scale the zoom to the change, and fix what a level turns up before widening.
  - Tests mirror the zooms: property → unit → crate integration → end-to-end.
  - **Prove a test fails before the fix by actually reverting the fix.** A test nobody has seen fail is not evidence.
  - **Rebasing is a review step, not a mechanical one.** Read every hunk you resolve, and the auto-merged hunks in the files you touched too.
- **Conformance numbers are measured, not claimed.** The W3C/OASIS suites (xmlconf, xsdtests) are fetched by `cargo xtask conformance`, and the pass rates are ratcheted in `xtask/baselines/`: a new failure fails CI, and a fixed test removes its line. Report the real numbers in `docs/target-app-parity.md`, with the known gaps in `docs/gaps.md`.
- **External PRs are held to safety, security and completeness before style.** Provenance first (no Altova-derived asset, no unapproved bundled schema, no copied GPL/AGPL code, behaviour from the specification and not from reading another implementation), then no new panic path or `unsafe`, then `--root` confinement on every path including ones reached by nested dispatch, opt-in-only MCP and control channel, no new outbound network, then a headless `automation` tool, tests at the right zoom and an honest `parity/` entry. Provenance is the part a contributor cannot be expected to know, and the only part that cannot be undone after merge.
- **Quality gates** before every commit: `cargo fmt --check`, clippy with `-D warnings`, the tests, `cargo xtask layers`, `cargo xtask assets`, and the wasm check.
  - **Scope builds and tests to the crates you touched** — `cargo clippy -p <crate> -j 2 --all-targets` and `cargo test -p <crate> -j 2`, under a `timeout`. A full `--workspace` build can lock up a development machine's CPU; it is CI's job and CI runs it on every PR. Run the workspace sweep locally only when you have a reason to and the machine is free.
- **Check for existing work before starting a fix or feature.** Search the issues and PRs, open and closed, with `gh issue list -R thorstenhirsch/xmlcraft --state all --search "<terms>"` and the same with `gh pr list` (try a few phrasings), and check whether a merged fix is already on `main` (`git merge-base --is-ancestor <commit> origin/main`). If one is, the report is likely from an older build: say so rather than fixing it again. If an open PR covers it, review or build on that PR instead of opening a competing one. If an issue exists, reference it in the PR (`Fixes #123`).
- **Commits and PRs:** `main` is the only integration branch and is **never pushed to directly**. Branch from an up-to-date `main` as `<type>/<slug>` (`feat/`, `fix/`, `docs/`, `chore/`, `refactor/`), one task id per commit (e.g. `M1.4: rowan CST parser`), commit only green states, and open a PR into `main` using `.github/pull_request_template.md`. Merge only when CI is green. Rebase your own branch the same day `main` moves under it, and never rewrite a contributor's branch.

## Decisions that are the user's
Stop and ask before: creating or pushing to a public repository, adding a bundled schema set or any licence outside the allowlist, adding outbound network access, adding an `unsafe` block, changing the licence or copyright line, committing a font or any other binary asset not already approved, and spending or configuring release secrets.

## Running and looking at the app
- `cargo run -p xmlcraft -- <file.xml>` opens the desktop app.
- `cargo run -p xmlcraft-cli -- run --script steps.json --root DIR` drives the engine headlessly through the automation tools (`xmlcraft-cli tools` lists them). Use it to check engine changes. `xmlcraft-cli mcp` is the opt-in MCP server (`AGENTS.md` §3).
- `cargo xtask fuzz --time 300` mutation-fuzzes parse/validate/infer/sample/format in child processes. Findings land in `fuzz-out/findings/` (git-ignored; never commit corpus-derived files). Turn every real finding into a small synthetic regression test before fixing it.
- `cargo xtask conformance <suite>` runs a W3C/OASIS suite and its ratchet. `cargo xtask oracle` compares verdicts with `xmllint`. `cargo xtask parity [--partial]` reports XMLSpy-parity progress from `parity/xmlspy-features.toml`. Update the entry when a feature ships.
- For UI work, **look at the result**. Either launch `xmlcraft --control FILE doc.xml` and use `xmlcraft-cli ui --control FILE screenshot --out x.png` (plus `inspect`, `click`, `key`, `type`, `command`, `set`), or take a headless shot with `cargo run -p xmlcraft-ui-egui --example shot`. Control-channel tests use kittest.
- Parallel agents: use a separate `CARGO_TARGET_DIR` per agent and separate git worktrees.

## Current bootstrap state
Tracked in `plan/STATUS.md`. At 0.0.x the workspace is being bootstrapped milestone by milestone (M0 → M5, see `docs/roadmap.md`); crates and `xtask` commands named in these documents arrive with the milestone that introduces them.
