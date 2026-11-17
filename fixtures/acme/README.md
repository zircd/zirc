# Acme

Acme is the made-up team everything zirc shows of itself uses: the phones' store
screenshots and the web client's shots. (zirc.dev's home page's
picture and the docs' terminals are another team of the same shape, `../home`.) It's defined here once, and each of those reads it. Nothing in it is a real
person, company or piece of data.

| File | What | Kept current by |
|---|---|---|
| `showcase.json` | The team and its morning: people, agents, channels, what was said | `showcase_test`, below |
| `coupon-before-after.png` | The picture a file in the morning shares | |

Screenshots made from it are retaken by hand, and nothing checks them: the phones' with
`bazel test //oss/apps/android:store_shots` and iOS's `StoreTests` (`oss/apps/ios/BUILD.bazel`).

## The showcase

`showcase.json` is what Zirc's store screenshots show, the same on every phone: a made-up
team, Acme, with five people, three of their coding agents, a few channels and what was
said in them, at fixed times. Nothing in it is a real person, company or piece of data.

It shows what Zirc is for: an agent saying what it's about to change, an overlap between
two agents, a person heading it off, a file, a CI landing with reactions and a failed build
with its Retry and Logs buttons, a direct message, a conversation with an agent (with a
code block and a file path), mentions for Activity, agents at work, and requests answered in
#decisions.

Messages are plain words, as people type them; `` `code` `` and a `code` block
(`{ "language", "text" }`) are what the core would read as code. A `failed` message is CI's
card, its `buttons` the actions it offers. A `request` is an agent's question to its owner as
zircd posts it in #decisions, its `text` the question, `yes` and `no` what each does, and
`answered` when its owner said yes. `mentions` are what names `me` across channels.

An agent's event (`intent`, `overlap`) is as zirc-chat's `Parts` has it: its `verb`, what it
was in `details` (an intent's task, empty for an overlap), and the files in `paths`, apart:
the phones draw the marker and verb, the paths, then the task on the line under.

The real core plays it with no server (zirc-app's `showcase`), so every app's screenshots
show what the app decides. What only a server or zirc.dev would bring, such as several
organisations, being offline, searching everywhere, the server's list of channels
(`browse`), recent searches (`searches`) or zirc.dev's profile, the core stages on request
(the mobile core's `stage_showcase`). An `overlap` is staged as the two agents' working
sets, which zirc-chat reads as both changing, as it does live.

- Android: `bazel test //oss/apps/android:store_shots` renders the Play screens and the
  design pass's (oss/apps/android/README.md).
- iOS: the App Store sizes, from the same file. Its design pass and the Mac's harness show
  it by the real core too: offline, or on a scripted network playing it
  (`oss/apps/mobile/swift/Fakes`).

A file's picture is beside it, as `picture` names it: `coupon-before-after.png`, the checkout
page's coupon field before and after the fix the conversation is about.

`screens` lists the screens to take, in order, with a caption each for the store listing.
`bazel test //fixtures/acme:showcase_test` checks it names only what it has.

## An agent's conversation and our machines' threads

`activity` is what the core's `agent_activity` gives for an agent session: its lines in time
order, each named by the conversation it is in (`in`) and its message `id`. A line in the
agent's own conversation is a direct message (no room); one in a channel or agents room has
that room as its chip. An agent with an empty list shows the empty case. `read_only` is what
replaces the composer for an agent that isn't `me`'s: the note, its button and the owner's
conversation the button opens. `machines` are `me`'s machines whose thread has messages
(`Your agents · mbp`, the conversation `maya-mbp` holds them) with the sessions on top.
