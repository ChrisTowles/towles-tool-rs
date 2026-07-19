# Plugin Distribution Options

> **Status note (2026-07-19):** `tt install` was removed in the CLI trim; the
> references to it below describe behavior that shipped before that. Plugin
> marketplace/MCP registration is moving to the app's setup flow (see the CLI
> redesign). The URL-keying analysis below still holds.

The `packages/core` plugin (`tt`) ships through a Claude Code **marketplace**. A
user runs `claude plugin marketplace add https://github.com/ChrisTowles/towles-tool`,
which fetches `.claude-plugin/marketplace.json` from that repo's root; that manifest
lists the `tt` plugin with `source: ./packages/core`. Users then install
`tt@towles-tool` (plugin `tt`, marketplace `towles-tool`). Note `tt install` also
adds that **same URL** (`https://github.com/ChrisTowles/towles-tool`, marketplace
name `towles-tool`) — so the Rust CLI already points existing behavior at the live
repo. The marketplace resolves purely by that GitHub URL + the manifest at its root;
nothing in the client hardcodes a local path.

This repo currently carries `packages/core` verbatim but has **no marketplace.json
at its root** and no configured git remote. Below are three ways to ship once the
rewrite takes over. **This is an options analysis, not a decision — Chris decides.**

## (a) Keep shipping from `ChrisTowles/towles-tool` until the `ttr`→`tt` cutover

- **Existing installs:** unchanged. Everyone stays on the live repo's marketplace;
  `claude plugin update` keeps pulling from it. `tt install`'s URL already matches.
- **Migration steps:** none now. Keep publishing plugin changes to the live repo
  until the hard cutover, then repoint in one move.
- **Risks:** plugin source and the Rust CLI live in two repos during migration, so
  `packages/core` here can drift from what users actually receive. Requires
  discipline to treat this copy as a mirror, not the source of truth, until cutover.

## (b) Move the marketplace to this repo now

- **Existing installs:** no change *until* a user removes/re-adds the marketplace or
  the old repo stops serving the manifest. Because the marketplace is keyed by URL,
  a different repo URL is a *different* marketplace — existing `tt@towles-tool`
  installs keep pointing at the old URL until manually migrated.
- **Migration steps:** add a git remote for this repo, publish it, add a root
  `.claude-plugin/marketplace.json` here, and update `tt install` + docs to the new
  URL. Communicate a re-add to existing users.
- **Risks:** URL churn splits the user base across two marketplaces; anyone who
  doesn't re-add is stranded on the old one. Also conflicts with the "no remote / no
  push" migration constraint on this repo today.

## (c) Dedicated plugin repo (plugin content separate from the CLI)

- **Existing installs:** same URL-change caveat as (b) — a new repo URL is a new
  marketplace requiring a re-add, unless the old repo redirects.
- **Migration steps:** create a standalone repo holding only `packages/core` +
  `marketplace.json`, publish it, repoint `tt install`/docs. This repo would drop
  its `packages/core` copy or keep it as a submodule/mirror.
- **Risks:** most moving parts and a third repo to maintain; decouples plugin
  releases from CLI releases (which can be a feature or a burden). Same stranded-user
  risk on the URL change.

## Cross-cutting notes

- Any option that changes the marketplace **URL** (b, c) is a breaking change for
  already-installed users, since Claude tracks marketplaces by URL. Option (a) is the
  only one with zero user action.
- Whatever is chosen must keep `tt install`'s marketplace URL and the published
  `marketplace.json` in agreement, or new installs and the CLI will disagree.
