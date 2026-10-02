#!/usr/bin/env python3
"""Seed an Azure DevOps sandbox project for the live ado tests.

Re-runnable. It gives the team two-week sprints around today (one past, the
current one, two future), a capacity for every member and a team day off in
the current sprint, then, once, an Epic > Feature/Issue > Story/Task tree
tagged agent-cli-e2e and a shared query over it. Run it before
AGENT_CLI_TEST_ADO=1 tests on a project nobody else uses: it writes.

  AZURE_DEVOPS_EXT_PAT=… scripts/ado-sandbox.py [--project NAME --process Agile]

Org, project and team come from [ado] in agent-cli's config.toml unless
given; --process creates the project when it is missing.
"""

import argparse
import base64
import datetime as dt
import json
import os
import pathlib
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

API = "api-version=7.1"
TAG = "agent-cli-e2e"


def config():
    path = os.environ.get("AGENT_CLI_CONFIG") or pathlib.Path(
        os.environ.get("XDG_CONFIG_HOME", pathlib.Path.home() / ".config")
    ) / "agent-cli" / "config.toml"
    return tomllib.loads(pathlib.Path(path).read_text()).get("ado", {})


class Ado:
    def __init__(self, org, project, pat):
        self.base = f"https://dev.azure.com/{org}"
        self.project = project
        self.auth = "Basic " + base64.b64encode(f":{pat}".encode()).decode()

    def call(self, method, url, body=None, kind="application/json"):
        url = url if url.startswith("https://") else f"{self.base}/{url}"
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(url, data, method=method)
        req.add_header("Authorization", self.auth)
        req.add_header("Content-Type", kind)
        try:
            with urllib.request.urlopen(req) as res:
                text = res.read().decode("utf-8-sig")
        except urllib.error.HTTPError as err:
            raise SystemExit(f"{method} {url}: {err.code} {err.read().decode('utf-8-sig')[:400]}")
        return json.loads(text) if text else {}

    def p(self, path):
        return f"{urllib.parse.quote(self.project)}/_apis/{path}"

    def t(self, team, path):
        return f"{urllib.parse.quote(self.project)}/{urllib.parse.quote(team)}/_apis/{path}"


def ensure_project(ado, process):
    names = [p["name"] for p in ado.call("GET", f"_apis/projects?{API}")["value"]]
    if ado.project in names:
        return
    if not process:
        raise SystemExit(f"no project {ado.project!r}; pass --process Agile|Scrum|Basic to create it")
    ids = {p["name"]: p["id"] for p in ado.call("GET", f"_apis/process/processes?{API}")["value"]}
    op = ado.call("POST", f"_apis/projects?{API}", {
        "name": ado.project,
        "description": "agent-cli live test sandbox",
        "visibility": "private",
        "capabilities": {
            "versioncontrol": {"sourceControlType": "Git"},
            "processTemplate": {"templateTypeId": ids[process]},
        },
    })
    status = "queued"
    for _ in range(60):
        status = ado.call("GET", op["url"])["status"]
        if status == "succeeded":
            print(f"created project {ado.project} ({process})")
            return
        if status in ("failed", "cancelled"):
            raise SystemExit(f"creating {ado.project}: {status}")
        time.sleep(2)
    raise SystemExit(f"creating {ado.project}: still {status} after two minutes")


def sprints(today):
    monday = today - dt.timedelta(days=today.weekday())
    # The current sprint starts on an even ISO week's Monday, so reruns agree.
    if monday.isocalendar().week % 2:
        monday -= dt.timedelta(days=7)
    for k in (-1, 0, 1, 2):
        start = monday + dt.timedelta(days=14 * k)
        yield f"Sprint {start.isoformat()}", start, start + dt.timedelta(days=11)


def ensure_sprints(ado, team, today):
    root = ado.call("GET", ado.p(f"wit/classificationnodes/iterations?$depth=1&{API}"))
    have = {c["name"]: c for c in root.get("children", [])}
    team_has = {i["path"] for i in ado.call("GET", ado.t(team, f"work/teamsettings/iterations?{API}"))["value"]}
    made = []
    for name, start, finish in sprints(today):
        node = have.get(name) or ado.call("POST", ado.p(f"wit/classificationnodes/iterations?{API}"), {
            "name": name,
            "attributes": {"startDate": f"{start}T00:00:00Z", "finishDate": f"{finish}T00:00:00Z"},
        })
        path = f"{ado.project}\\{name}"
        if path not in team_has:
            ado.call("POST", ado.t(team, f"work/teamsettings/iterations?{API}"), {"id": node["identifier"]})
        made.append((name, node["identifier"], start, finish))
    print("sprints:", ", ".join(m[0] for m in made))
    return made


def ensure_capacity(ado, team, made, today):
    members = ado.call("GET", f"_apis/projects/{urllib.parse.quote(ado.project)}/teams/{urllib.parse.quote(team)}/members?{API}")["value"]
    for name, iteration, start, finish in made:
        for i, member in enumerate(members):
            who = member["identity"]
            off = [] if i else [{"start": f"{start + dt.timedelta(days=2)}T00:00:00Z",
                                 "end": f"{start + dt.timedelta(days=3)}T00:00:00Z"}]
            ado.call("PATCH", ado.t(team, f"work/teamsettings/iterations/{iteration}/capacities/{who['id']}?{API}"), {
                "activities": [{"name": "Development", "capacityPerDay": 6}],
                "daysOff": off,
            })
        if start <= today <= finish:
            friday = start + dt.timedelta(days=4)
            ado.call("PATCH", ado.t(team, f"work/teamsettings/iterations/{iteration}/teamdaysoff?{API}"), {
                "daysOff": [{"start": f"{friday}T00:00:00Z", "end": f"{friday}T00:00:00Z"}],
            })
    print(f"capacity: {len(members)} member(s), 6 h/day; one day off for the first; a team day off this sprint")


def types_of(ado):
    names = {t["name"] for t in ado.call("GET", ado.p(f"wit/workitemtypes?{API}"))["value"]}
    story = next(n for n in ("User Story", "Product Backlog Item", "Requirement", "Issue") if n in names)
    feature = "Feature" if "Feature" in names else None
    return story, feature


def create(ado, kind, fields, parent=None):
    ops = [{"op": "add", "path": f"/fields/{k}", "value": v} for k, v in fields.items()]
    if parent:
        ops.append({"op": "add", "path": "/relations/-", "value": {
            "rel": "System.LinkTypes.Hierarchy-Reverse", "url": parent["url"]}})
    return ado.call("POST", ado.p(f"wit/workitems/${urllib.parse.quote(kind)}?{API}"), ops,
                    "application/json-patch+json")


def ensure_items(ado, made):
    wiql = {"query": f"SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] = @project "
                     f"AND [System.Tags] CONTAINS '{TAG}'"}
    if ado.call("POST", ado.p(f"wit/wiql?$top=1&{API}"), wiql)["workItems"]:
        print(f"work items: already seeded (tag {TAG})")
        return
    story, feature = types_of(ado)
    points = "Microsoft.VSTS.Scheduling.StoryPoints" if story == "User Story" else "Microsoft.VSTS.Scheduling.Effort"
    sprint = {when: f"{ado.project}\\{name}" for when, (name, *_) in zip(("past", "now", "next", "later"), made)}
    base = {"System.Tags": TAG}
    epic = create(ado, "Epic", base | {"System.Title": "Checkout v2"})
    mid = create(ado, feature, base | {"System.Title": "Saved carts"}, epic) if feature else epic
    rows = [
        ("Save a cart for later", "now", 5, ["Cart table migration", "Save endpoint"]),
        ("Restore a saved cart", "now", 3, ["Restore endpoint"]),
        ("Share a cart by link", "now", 8, []),
        ("Expire carts after 30 days", "next", 2, []),
        ("Cart analytics", "later", 5, []),
    ]
    for title, when, size, tasks in rows:
        item = create(ado, story, base | {"System.Title": title, "System.IterationPath": sprint[when], points: size}, mid)
        for task in tasks:
            create(ado, "Task", base | {"System.Title": task, "System.IterationPath": sprint[when],
                                        "Microsoft.VSTS.Scheduling.RemainingWork": 6}, item)
    create(ado, "Bug" if story != "Issue" else "Issue",
           base | {"System.Title": "Cart total ignores coupons", "System.IterationPath": sprint["past"]})
    print(f"work items: an epic, {len(rows)} {story}s with tasks and a bug, tagged {TAG}")


def ensure_query(ado):
    folder = ado.call("GET", ado.p(f"wit/queries/Shared%20Queries?$depth=1&{API}"))
    if any(c["name"] == "agent-cli e2e open" for c in folder.get("children", [])):
        return
    ado.call("POST", ado.p(f"wit/queries/Shared%20Queries?{API}"), {
        "name": "agent-cli e2e open",
        "wiql": f"SELECT [System.Id], [System.Title], [System.State] FROM WorkItems "
                f"WHERE [System.TeamProject] = @project AND [System.Tags] CONTAINS '{TAG}' "
                f"AND [System.State] NOT IN ('Done', 'Closed', 'Removed', 'Resolved') ORDER BY [System.Id]",
    })
    print("query: Shared Queries/agent-cli e2e open")


def main():
    cfg = config()
    ap = argparse.ArgumentParser(description="Seed an Azure DevOps sandbox project for the live ado tests.")
    ap.add_argument("--org", default=cfg.get("org"))
    ap.add_argument("--project", default=cfg.get("project"))
    ap.add_argument("--team", help="default: '[ado] team' for its project, else '<project> Team'")
    ap.add_argument("--process", help="create the project with this process when missing")
    args = ap.parse_args()
    pat = os.environ.get("AZURE_DEVOPS_EXT_PAT")
    if not (args.org and args.project and pat):
        sys.exit("needs [ado] org and project (or --org/--project) and AZURE_DEVOPS_EXT_PAT")
    org = args.org.rstrip("/").rsplit("/", 1)[-1]
    configured = cfg.get("team")
    configured = configured[0] if isinstance(configured, list) else configured
    team = args.team or (configured if args.project == cfg.get("project") else None) or f"{args.project} Team"
    ado = Ado(org, args.project, pat)
    ensure_project(ado, args.process)
    today = dt.date.today()
    made = ensure_sprints(ado, team, today)
    ensure_capacity(ado, team, made, today)
    ensure_items(ado, made)
    ensure_query(ado)


if __name__ == "__main__":
    main()
