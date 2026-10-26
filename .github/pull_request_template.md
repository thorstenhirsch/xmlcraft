<!--
Thanks for contributing to XmlCraft.

Base this on `main` — it is the only integration branch. Rebase the same day `main` moves under
your branch, so the work doesn't die of conflicts.
Delete any section below that does not apply.
-->

## What this changes

<!-- What behaviour is different afterwards, and why it is worth doing. If it fixes an
     issue, say `Fixes #123`. -->

## How you verified it

<!-- The commands you ran and what they said. Per-crate is fine and preferred:
     `cargo test -p <crate>`. CI runs the full workspace on every PR, so you do not
     need to locally. -->

- [ ] `cargo fmt --check`
- [ ] clippy with `-D warnings` on the crates I changed
- [ ] tests pass on the crates I changed
- [ ] `cargo xtask layers`, `cargo xtask assets` and `cargo xtask schemas` (if you touched crate boundaries, any asset or any bundled schema)

**Tests.** New behaviour needs a test at the right scale — a property test, a unit test, a
crate integration test or an end-to-end one. For a bug fix, **please check that the test
fails before your fix** by reverting the fix and watching it go red. A test nobody has seen
fail is not evidence that it pins anything.

## Never-crash

Every XML file, schema, catalog, downloaded resource, script, MCP call, settings file and
keystroke is untrusted input (`AGENTS.md` §5).

- [ ] No new `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!` outside tests, unless it is
      provably infallible with a comment saying why
- [ ] No indexing or slicing with input-derived positions (`get`, and strings only at char
      boundaries); checked or saturating arithmetic; allocations sized by input are capped;
      recursion and entity expansion are bounded
- [ ] No `unsafe`
- [ ] Any crash this fixes has a small synthetic regression test

## Provenance

This is the part that cannot be undone after a merge, and the part a new contributor is
least likely to know about. Please read `AGENTS.md` §1 and §2 before adding any asset or schema.

- [ ] No asset from an Altova product (XMLSpy and friends), and no Altova icon, graphic,
      catalog or sample file recreated, traced or imitated
- [ ] No font files committed (the Latin UI fonts in `assets/fonts/` are the only ones)
- [ ] No GPL/AGPL code copied in — libxml2/`xmllint`, Xerces, Saxon and the Rust XSD crates may
      only be used as black-box oracles
- [ ] Behaviour comes from the specification or black-box observation, **not** from reading
      another implementation's source
- [ ] Every new asset has an `ATTRIBUTION.toml` entry and `cargo xtask assets` passes
- [ ] Bundled schemas: unmodified upstream files, on the licence allowlist (`AGENTS.md` §1.6),
      and the owner approved the new set

## Security

- [ ] `--root` confinement still holds on every path, including ones reached by nested
      tool dispatch
- [ ] MCP and the UI control channel stay opt-in, loopback-only and token-authenticated
- [ ] No new outbound network access (that decision is the owner's; `AGENTS.md` §4)

## Parity and reach

- [ ] `parity/xmlspy-features.toml` updated in this PR, with `notes` saying what is *still*
      missing — `shipped` means "exists and tested", not "as good as XMLSpy"
- [ ] Conformance numbers in `docs/target-app-parity.md` still true (xmlconf, xsdtests)
- [ ] Reachable headlessly through an `automation` tool where that applies (`AGENTS.md` §3),
      so agents can drive it and not just the UI
- [ ] Docs updated if this changes a documented behaviour (`docs/`, crate README)

## Anything a reviewer should look at first

<!-- The judgement call you are least sure about, a deliberate restraint you want pinned,
     or a behaviour change existing users will notice. Say so here rather than leaving it
     to be found. -->
