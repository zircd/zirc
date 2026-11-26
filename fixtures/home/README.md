# The team

`home.json` is the made-up team everything zirc shows of itself uses: zirc.dev's home page's
picture, the store screenshots, the design pass's shots, the web client's shots and the
apps' tests. It's defined here once, and each of those reads it. A team here called Zirc, on
a morning a senior team would know: a latency fix with its numbers, two agents in one file,
`main` going red and the agent holding the fixer role taking the fix, and the agents working
round each other in the organisation's `#agents`, with CI, while Henrik is out. `#backend`
keeps the people's talk, as every repository's agents and CI speak in one `#agents` (the
user's choice, 20 Nov 2026). The team has one repository, so no line in `#agents` names it.
Nothing in it is a real person, company or piece of data.

It replaced Acme, the team of the coupon field (the user's choice, 25 Nov 2026: one team and
one story, the home page's).

| File | What | Kept current by |
|---|---|---|
| `home.json` | The team and its morning: people, agents, channels, what was said | `home_test`, below |
| `replay-p99-before-after.png` | The picture a file in the morning shares | |
| `terminals.json` | The same morning as `zirc` and `zircd` print it | `//crates/zirc:zirc_terminals_test` |

Screenshots made from it are retaken by hand, and nothing checks them: the home page's with
`bazel run //web/art:home-shots` (the Mac's window in `#agents` and the iPhone's screen in
`#backend`), the stores' with
`bazel test //oss/apps/ios:store_69 //oss/apps/ios:store_65 //oss/apps/android:store_shots`.

## What it holds

Six people, seven of their coding agents (four of them Henrik's, who is `me`; Jari's has
said nothing yet, the empty case), a few channels
and what was said in them, at fixed times. It shows what Zirc is for: an agent saying what
it's about to change, an overlap between two agents, a person heading it off, a file, a CI
landing with reactions and failed builds with their Retry and Logs buttons, a direct
message, conversations with an agent (with a code block and a file path; replay-metrics'
has a long message and a long code block, which fold), mentions for Activity, agents at
work, and requests in #decisions, one still open and irreversible.

Messages are plain words, as people type them; `` `code` `` and a `code` block
(`{ "language", "text" }`) are what the core would read as code. A `failed` message is CI's
card, its `buttons` the actions it offers. A `request` is an agent's question to its owner as
zircd posts it in #decisions, its `text` the question, `yes` and `no` what each does,
`answered` when its owner said yes, and `irreversible` when a yes can't be undone. `mentions`
are what names `me` across channels.

An agent's event (`intent`, `overlap`) is as zirc-chat's `Parts` has it: its `verb`, what it
was in `details` (an intent's task, empty for an overlap), and the files in `paths`, apart:
the phones draw the marker and verb, the paths, then the task on the line under.

The real core plays it with no server (zirc-app's `showcase`), so every app's screenshots
show what the app decides. What only a server or zirc.dev would bring, such as several
organisations (`organisations`, Fjord the second), being offline, searching everywhere, the
server's list of channels (`browse`), recent searches (`searches`) or zirc.dev's profile,
the core stages on request (the mobile core's `stage_showcase`). An `overlap` is staged as
the two agents' working sets, which zirc-chat reads as both changing, as it does live.

A file's picture is beside it, as `picture` names it: `replay-p99-before-after.png`, history
replay's p99 before and after the index the morning is about, which Raj shares in `#general`.

`screens` lists the home page's two shots; `store` the stores' six (the last the Messages
tab, `dms` as Android names it), in order, with a caption each for the store listing, which
both phones' store shots take. The additions the tests and the design pass need (the second
organisation, `#general`'s picture, replay-metrics' conversation and its overlap with
wal-compaction, Jari's silent agent, `browse`, `searches`) are outside what those shots
show. `#clients` has no line, for the empty channel.
`bazel test //fixtures/home:home_test` checks it names only what it has.

## An agent's conversation and our machines' threads

`activity` is what the core's `agent_activity` gives for an agent session: its lines in time
order, each named by the conversation it is in (`in`) and its message `id`. A line in the
agent's own conversation is a direct message (no room); one in a channel has that room as
its chip. `read_only` is what replaces the composer for an agent that isn't `me`'s: the
note, its button and the owner's conversation the button opens. `machines` are `me`'s
machines whose thread has messages (`Your agents · mbp`, the conversation `henrik-mbp`
holds them) with the sessions on top.

## The terminals

`terminals.json` is the same team's morning as `zirc` and `zircd` print it, for zirc.dev's
`<Terminal>` on the home page and in the docs: Henrik's and Raj's machines and CI's, in
`zirc/backend`. `//crates/zirc:zirc_terminals_test` runs it for real and fails when zirc
prints something else; `bazel run //fixtures/home:terminals` brings the file up to date.
