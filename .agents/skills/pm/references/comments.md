# Comments and ticket text

People read these on a phone between meetings. A model's default is an
info dump; these rules replace it.

## Rules

- Start with `Jacob's agent:`.
- One or two sentences. The first words say what changed or what is needed.
- Give one reason, the strongest. Link details rather than list them.
- Use names people know (PR 412, Sam, the orders export). Never field
  reference names, timestamps, revs, or ids nobody uses.
- No headers, bullets or tables, except a recorded decision with several parts.
- No preamble ("I have analyzed…") and no hedging ("it appears that…").
- `@mention` only a teammate who must act.

Ticket descriptions follow the same voice: the requester line, two or three
sentences of what and why, then `## Open questions` if any. Acceptance
criteria are a short list of observable checks.

## Examples

**Closing a PBI**
- No: "State transition analysis: PR !412 (contoso-api, completed
  2026-10-02T14:03:11Z, 3 approvals) linked; branch has 14 commits;
  acceptance criteria 1–3 verified; updating System.State Committed → Done."
- Yes: "Jacob's agent: Marking this Done. PR 412 merged and QA passed."

**Waiting on a stakeholder**
- No: "Blocked pending stakeholder input regarding scope ambiguity in
  acceptance criterion 2 (archived account handling) …"
- Yes: "Jacob's agent: Waiting on Sam to confirm whether archived accounts
  are included. Jacob is asking."

**Recording an answer**
- No: "Update: stakeholder feedback received. Summary of discussion: …"
- Yes: "Jacob's agent: Per Sam, 2026-10-03 (call): weekly is fine, CSV only."

**Ops work done**
- No: "Executed remediation script against production database instance;
  see attached output for details of affected records."
- Yes: "Jacob's agent: Done. Updated 214 rows in Orders on contoso-sql, Oct 3."

## Adding examples

When the user rewrites a proposed comment, add the pair here as No/Yes
under a short heading, with every real name, project and server replaced
by a placeholder (Sam, contoso-sql, PR 412). This repo is public.
