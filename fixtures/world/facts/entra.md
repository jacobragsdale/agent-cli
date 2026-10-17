# entra in the contoso world

What the recordings in `http/entra.json` (Microsoft Graph) say, consistent
with every other domain's facts. Keep a new recording consistent with these,
and add a line when you add a fact.

- The tenant is `…a001` (facts/k8s.md), domain `contoso.example`. The az
  login is `ops@contoso.example` (Contoso Ops, object id `…00e1`, as
  `scripts/fake/az`'s token says; ADO's Jane Doe is `@me` in ADO's own id
  space), a member holding Reports Reader, in sg-contoso-platform. Its token
  carries the Azure CLI's scopes (AuditLog.Read.All,
  Directory.AccessAsUser.All, Group.ReadWrite.All, User.ReadWrite.All), and
  `doctor entra` passes every check.
- Users: `dana@contoso.example` (Dana Reyes, `…e101`, Platform lead, Global
  Administrator, no manager; arm trace 4's deleter), `sam@contoso.example`
  (Sam Lee, `…e102`, SRE, mail `sam@contoso.com` as ADO knows him, managed by
  dana), `alex@contoso.example` (Alex Kim, `…e103`, Finance, disabled).
- Groups: `…f001` sg-contoso-platform (security, Contributor on contoso-prod
  in facts/arm.md, owner dana, members dana, ops and `…f003`; 3 members, 4
  through nesting); `…f003` sg-contoso-oncall, holding sam, whom dana added
  at 2026-09-25T10:02:13Z (the audit log); `…f004` sg-contoso-sre, dynamic
  (`user.department -eq "SRE"`, On), holding sam.
- The four managed identities' service principals (facts/arm.md), each with
  its ARM id in `alternativeNames`: the kubelet's `…c003`, the CSI add-on's
  `…c005`, the notifier's `…b0f2`, Airflow's `…a1f2` (app id `…a1f1`; no app
  roles, no grants, in no group, as facts/lake.md says). `arm role list`
  names them, and sg-contoso-platform, from one `getByIds` a call.
- App registrations (`app list`, four): `contoso-ado-deploy` (app id
  `…5d01`, object `…5d02`, service principal `…5d03`, owner dana, asking
  for Graph's User.Read) is ADO's connection `contoso-prod` (facts/ado.md).
  Its secret `sc-contoso-prod` (key `…5d0a`) expired 2026-09-20T00:00:00Z;
  dana added `sc-contoso-prod-2026` (key `…5d0b`, ends 2027-03-17) at
  2026-09-18T15:20:41Z, and nobody gave it to the connection: since
  2026-09-29T06:00:00Z its sign-ins to the Windows Azure Service Management
  API fail with AADSTS7000222 presenting `…5d0a` (06:00:14, 06:00:41,
  09:30:07, from 203.0.113.40). `contoso-reports` (`…5e01`, object `…5e02`)
  holds the certificate `CN=contoso-reports`, ending 2026-10-12; alex is its
  only owner, so `app get`'s note goes to the audit log, which holds nothing
  for it in 30 days. `contoso-ado-dev` (`…5f01`, object `…5f02`, owner sam)
  has no secret: ADO's `contoso-dev` signs in by its federated credential
  (subject `sc://contoso/Fabrikam/contoso-dev`). `contoso-finance-close`
  (`…a2f1`, object `…a2f3`, service principal `…a2f2`, owner dana) is the
  ledger's writer in facts/lake.md, its secret ending 2027-06-30.
- `app list --expires-within 30d` finds contoso-ado-deploy and
  contoso-reports.
- Directory roles: Global Administrator held by dana alone (active; PIM is
  not read), Reports Reader by ops.
