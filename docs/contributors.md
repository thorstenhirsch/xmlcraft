# Contributors in the About window

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** the generator and the About window exist (M4.9); initial (copied from PdfCraft's decision, adapted)

**About ▸ Contributors** credits everyone who contributed to XmlCraft, and **About ▸ Models** credits the AI models named in `Co-Authored-By` trailers.

## Decision

- Two files, both **compiled into the binary** (nothing is read from disk or the network at run time):
  - `contributors/contributors.json`: commit stats per GitHub username. **Generated**; never hand-edit. `cargo xtask contributors --write` makes it from `git log` (a commit's author is told by a GitHub `noreply` address, or asked of GitHub with `gh`; bots are neither contributors nor models; whoever cannot be told is counted as unattributed) and bakes in the names people chose: a `people.toml` entry is used only when **every line of it was last written by a commit of that person** (`git blame`), so nobody is credited by a name someone else gave them. The About window (Help ▸ About XmlCraft) reads it and the NOTICE from the binary.
  - [`contributors/people.toml`](../contributors/people.toml): the names people chose for themselves. **Each contributor adds only their own entry**, in their own PR.
- Each contributor gets one line: GitHub username (always), display name (opt-in, or their synced GitHub profile name), real name (only if they told us), merged PRs, commits, lines added and deleted, first and last commit date. A missing name falls back to `@username`.

## Credit yourself

Add your entry to `contributors/people.toml` in a PR you commit yourself (every field optional):

```toml
[people.your-github-username]
real_name = "Your Name"
display_name = "Nickname"
sync_github_name = true   # use the name on your public GitHub profile as display name
```

## Revision history

- 2026-10-27: created.
- 2026-10-29: `cargo xtask contributors` and the About window (M4.9).
