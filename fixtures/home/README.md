# The home page's team

`home.json` is the team in zirc.dev's home page's picture: a made-up team, here called Zirc,
on a morning a senior team would know. A latency fix with its numbers, two agents in one
file, `main` going red, a workflow (`@main_bot`) handing the fix to an agent, and that agent
asking before it lands ahead of the queue. Nothing in it is a real person or piece of data.

It has the shape of Acme's showcase (`../acme/README.md`, which says what each field is) and
the same core plays it (zirc-app's `showcase`), so the picture is what the apps draw. It adds
one kind of line: a `run`, a workflow's nick (one of `apps`) giving a step out, with the
run's `card` as it stands, the `step` and whom it went `to`, as `x-zirc/run` carries them.

`bazel run //web/art:home-shots` retakes the page's two shots from it, the Mac's window in
`#backend` and the iPhone's screen in `#decisions`. `bazel test //fixtures/home:home_test`
checks it names only what it has.

`terminals.json` is the same team's morning as `zirc` and `zircd` print it, for zirc.dev's
`<Terminal>` on the home page and in the docs: Henrik's and Raj's machines and CI's, in
`zirc/backend`. `//crates/zirc:zirc_terminals_test` runs it for real and fails when zirc
prints something else; `bazel run //fixtures/home:terminals` brings the file up to date.

Acme stays the team of the store screenshots until they move to this one.
