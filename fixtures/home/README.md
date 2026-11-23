# The home page's team

`home.json` is the team in zirc.dev's home page's picture: a made-up team, here called Zirc,
on a morning a senior team would know. A latency fix with its numbers, two agents in one
file, `main` going red, a workflow (`@main_bot`) handing the fix to an agent, and the agents
working round each other in the organisation's `#agents`, with CI, while Henrik is out:
`#backend` keeps the people's talk, as every repository's agents and CI speak in one
`#agents` (the user's choice, 20 Nov 2026). The team has one repository, so no line in
`#agents` names it. Nothing in it is a real person or piece of data.

It has the shape of Acme's showcase (`../acme/README.md`, which says what each field is) and
the same core plays it (zirc-app's `showcase`), so the picture is what the apps draw. It adds
one kind of line: a `run`, a workflow's nick (one of `apps`) giving a step out, with the
run's `card` as it stands, the `step` and whom it went `to`, as `x-zirc/run` carries them.

`bazel run //web/art:home-shots` retakes the page's two shots from it, the Mac's window in
`#agents` and the iPhone's screen in `#backend`. `bazel test //fixtures/home:home_test`
checks it names only what it has.

`terminals.json` is the same team's morning as `zirc` and `zircd` print it, for zirc.dev's
`<Terminal>` on the home page and in the docs: Henrik's and Raj's machines and CI's, in
`zirc/backend`. `//crates/zirc:zirc_terminals_test` runs it for real and fails when zirc
prints something else; `bazel run //fixtures/home:terminals` brings the file up to date.

The store screenshots play this team too (the user's choice, 23 Nov 2026): `store` lists the
six (the last the Messages tab, `dms` as Android names it), each as a screen of `screens` says it, which both phones' store shots take in that order
(`bazel test //oss/apps/ios:store_69 //oss/apps/ios:store_65 //oss/apps/android:store_shots`).
Acme stays the team of the design pass's shots and the apps' tests.
