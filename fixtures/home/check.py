"""Checks the team names only what it has: people, agents and conversations."""
import json
import os
import sys

show = json.load(open(sys.argv[1]))
people = {p["nick"] for p in show["people"]}
agents = {a["conversation"] for a in show["agents"]}
channels = {c["name"] for c in show["channels"]}
# Who may speak: its people, its agents and CI.
speakers = people | agents | {"zirc-ci"}
assert show["me"] in people, "me isn't one of the people"
assert show["org"]["members"] == len(people), "the org's members aren't its people"
# The organisations the phone is signed in to, the team's own first and shown: the one this file is.
organisations = show.get("organisations", [])
assert organisations and organisations[0]["name"] == show["org"]["name"], "the team's own isn't the first organisation"
for organisation in organisations:
    assert organisation["handle"] == show["me"], f"{organisation['org']}'s handle isn't me"
for agent in show["agents"]:
    assert agent["owner"] in people, f"{agent['conversation']}'s owner isn't one of the people"
# Our machines' threads, `Your agents · mbp`: what was said to every agent there.
machines = {m["conversation"] for m in show.get("machines", [])}
for machine in show.get("machines", []):
    assert machine["owner"] == show["me"], f"{machine['conversation']} isn't our machine"
    for session in machine["sessions"]:
        assert session in agents, f"{session} isn't an agent"
for name, messages in show["conversations"].items():
    assert name in channels or name in people or name in agents or name in machines, f"{name} is neither a channel, a person, an agent session nor a machine"
    ids = set()
    for message in messages:
        assert message["from"] in speakers, f"{message['id']} is from no one known"
        assert "reply_to" not in message or message["reply_to"] in ids, f"{message['id']} answers no earlier message"
        ids.add(message["id"])
        picture = message.get("picture")
        assert picture is None or os.path.exists(os.path.join(os.path.dirname(sys.argv[1]), picture)), f"{picture} isn't beside the team's file"
        assert message["kind"] in ("message", "file", "intent", "overlap", "landed", "failed", "request"), f"{message['id']} is a {message['kind']}"
        # A request is an agent's, asked of its owner: what a yes does and what a no does.
        assert message["kind"] != "request" or (message["from"] in agents and message.get("text") and message.get("yes") and message.get("no")), f"{message['id']} is a request without an agent, a question, a yes or a no"
        code = message.get("code")
        assert code is None or (code.get("language") and code.get("text")), f"{message['id']}'s code needs a language and text"
        for button in message.get("buttons", []):
            assert button.get("action") and button.get("label"), f"{message['id']} has a button without an action or label"
for mention in show.get("mentions", []):
    assert mention["channel"] in channels, f"mention {mention['id']} is in no channel"
    assert mention["from"] in people, f"mention {mention['id']} is from no one known"
    assert f"@{show['me']}" in mention["text"], f"mention {mention['id']} doesn't name me"
for dm in show["dms"]:
    assert dm["with"] in people, f"a DM with {dm['with']}, who isn't one of the people"
# An agent session's conversation: its activity from the rooms and its direct messages with the
# person, in time order, each line named by the conversation it is in; and, for an agent that
# isn't ours, what replaces the composer.
for agent, refs in show.get("activity", {}).items():
    assert agent in agents, f"activity of {agent}, who isn't an agent"
    times = []
    for ref in refs:
        found = [m for m in show["conversations"].get(ref["in"], []) if m["id"] == ref["id"]]
        assert found, f"{agent}'s activity names {ref['id']}, which {ref['in']} hasn't"
        times.append(found[0]["at"])
    assert times == sorted(times), f"{agent}'s activity isn't in time order"
for agent, note in show.get("read_only", {}).items():
    assert agent in agents and not agent.startswith(show["me"] + "-"), f"{agent} is ours, not read-only"
    assert note["dm"] in people, f"{agent}'s owner {note['dm']} isn't one of the people"
# The store's screenshots (`store`) open what's there too.
for screen in show["screens"] + show.get("store", []):
    opens = screen.get("open")
    assert opens is None or opens in show["conversations"] or opens in show.get("activity", {}), f"{screen['name']} opens nothing"
print("home: ok")
