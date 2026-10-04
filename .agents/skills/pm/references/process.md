# Process: Scrum, as this team runs it

## Hierarchy

```
Epic → Feature → PBI or Bug   (acceptance criteria, priority, requester line)
                   ├─ dev Task(s)
                   └─ QA Task(s)
```

Acceptance criteria live on PBIs and Bugs only. A Task has a description.
Every PBI and Bug has a parent Feature; flag one that has none.

| Type | States |
|---|---|
| Epic, Feature | New → In Progress → Done |
| PBI, Bug | New → Approved → Committed → Done |
| Task | To Do → In Progress → Done |
| any | Removed, which only the user sets |

## Requester line

The first line of a PBI or Bug description:
`Requested by <name>, <YYYY-MM-DD> (<call|email|Teams|meeting>)`.
Stakeholders rarely write in ADO, so this line is the only record of who asked.

## Ready (New → Approved)

A PBI or Bug is ready when it has acceptance criteria, a priority, the
requester line, and no `needs-info` tag. Propose Approved then, never before.

## Signals

Weigh these together. One signal alone never moves a state.

- the linked PRs' status (`workitem get` → `pull_requests`, then `pr get`)
- branches and commits naming the ticket
- pipeline runs on those PRs
- comments, and what the user said in this session
- the children's states

PR links are evidence, not proof. A merged PR can leave work undone, and
ops work (prod data updates, server maintenance) has no PR at all.

## State rules

- **Approved → Committed**: the PBI is in the current sprint, or one of its
  Tasks is In Progress. Flag a Committed PBI in no sprint, and a sprint PBI
  still New or Approved.
- **Task To Do → In Progress**: a branch, PR or commit names it, or the user
  says work started.
- **Dev Task → Done**: its linked PRs are merged and nothing in its
  description is left, or for ops work, a comment records what was done.
- **QA Task → Done**: only when the user says so or a QA comment says it
  passed. PR signals never close a QA Task.
- **PBI or Bug → Done**: every child Task is Done and the acceptance
  criteria look covered. If a Task is open, flag it; don't propose Done.
- **Feature, Epic**: In Progress once any child is Committed or In Progress;
  Done once every child is Done.

## Evidence for work without a PR

A prod data update or a maintenance Task closes with a comment saying what
was run, where, when, and the result:
`Jacob's agent: Done. Updated 214 rows in Orders on contoso-sql, Oct 3.`
Propose Done only when such a comment exists or the user gives the facts.

## Priority

The field takes 1, 2 or 3. A prod issue defaults to 1. Everything else gets
a proposed priority with its one reason, which the user confirms or changes
in the approval. Priority is the bucket; backlog order is the user's.

## Edge cases

- **PR with no ticket**: flag it. Name a likely ticket if the branch, title
  or text points to one (`agent-cli ado pr link PR --workitem ID` on approval).
- **Abandoned PR on an In Progress Task**: flag it; never close the Task.
- **Several PRs on one ticket**: Done needs all of them complete.
- **Duplicate**: propose `relation create DUP --duplicate-of ORIGINAL`,
  moving the requester line and open questions to the original, then Done
  on the duplicate with a comment naming the original.
- **Scope growth**: propose a new PBI rather than rewriting acceptance
  criteria on a Committed one.
- **Blocked**: a ticket blocker is `relation create ID --blocked-by OTHER`;
  an outside blocker (vendor, access) is the `blocked` tag plus a comment
  naming it.
- **Stakeholders disagree**: flag both positions with their requester lines;
  the user decides.

## Stale thresholds

Defaults until the user sets others. Each becomes a flag, not a change.

| Condition | Flag after |
|---|---|
| Committed or In Progress, no change | 5 working days |
| New, never triaged | 14 days |
| `needs-info` with no answer | 7 days |
| Assigned to someone not in `agent-cli ado person list` | at once |
