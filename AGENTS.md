# Working here
- Design first: `DESIGN.md` is the design, and its §14 is the order of work, one slice at a time, each approved before it starts. Build one thing at a time.
- This repository and Epimemer share one Claude Code session; the Epimemer checkout is `../epimemer`. Hand large coding chunks and routine runs (tests, gates) to an Opus subagent. A subagent that writes memory claims its own judge and passes its token.
- Branch in this checkout, no worktrees. Never merge, push, tag or publish without asking.
- Commit messages: short plain subjects, no attribution trailers.

# Code
- Rust and Bevy 0.19, native and browser from one crate: the browser build (`just wasm`, `trunk serve`) must keep passing `just check-wasm`. Bevy UI for menus and panels.
- The only coupling to Epimemer is the contract in the Epimemer repository's `dev-docs/VIZ_CONTRACT.md`: HTTP and WebSocket JSON from the hub. Nothing here imports Epimemer, and nothing here depends on a Petritype crate until the activity bar (`DESIGN.md` §13).
- Read-only client: it records no verdict and changes no setting on the server.
- No global mutable state; pass configuration as values and keep state in Bevy resources and components.
- Gates before hand-over: `just check` (the suite, clippy with warnings denied, `cargo fmt --check`); `just` lists the recipes.

# Writing
- Plain English: technical terms yes, jargon no. Commas or a second sentence, never em-dashes. Say what is true rather than what is not. Name the thing, never an issue number.
- Epimemer's vocabulary: topic node, metacontext, source node. A tag is only the string in a tag list.

# Memory
- Epimemer graph `epimemer-dev`, tag `epimemer-bevy`, metacontext `the-real`. Stores stay high level, never verbatim code or docs. Read the server's guide before nontrivial memory work.
