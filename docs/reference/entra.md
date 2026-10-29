# entra — Microsoft Entra ID (12 commands)

| Command | Effect | Summary |
|---|---|---|
| [`entra principal list`](#entra-principal-list) | read | Name the users, groups, apps or identities behind GUIDs, in one call |
| [`entra principal diff`](#entra-principal-diff) | read | Compare two users or identities: groups (nested too), directory and Azure roles |
| [`entra user get`](#entra-user-get) | read | Show an Entra user: UPN, enabled, manager, groups (nested) and directory roles |
| [`entra group list`](#entra-group-list) | read | List Entra groups by name, or the groups a user or service principal is in |
| [`entra group get`](#entra-group-get) | read | Show an Entra group: kind, dynamic rule, owners, groups it is in, member counts |
| [`entra member list`](#entra-member-list) | read | List who is in an Entra group, nested groups included, and through which |
| [`entra app list`](#entra-app-list) | read | List Entra app registrations and when their credentials expire (no values) |
| [`entra app get`](#entra-app-get) | read | Show an Entra app registration: credentials (no values), owners, federation |
| [`entra serviceprincipal get`](#entra-serviceprincipal-get) | read | Show an enterprise app or identity in Entra: the app roles and scopes granted it |
| [`entra signin list`](#entra-signin-list) | read | List Entra sign-ins of a user, service principal or managed identity |
| [`entra audit list`](#entra-audit-list) | read | List the Entra audit log: who added or removed members, credentials, roles |
| [`entra role list`](#entra-role-list) | read | List Entra directory roles held: who is Global Administrator, what X holds |

### entra principal list

```text
agent-cli entra principal list — Name the users, groups, apps or identities behind GUIDs, in one call
 *<ids> str[]  Object ids (arm role list's principal_id, a lake ACL's entries), app (client) ids, UPNs, or @me
  --limit int  (default 50)
Returns: [{id,type,name,upn,app_id,sp_type,arm,enabled}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra principal list 00000000-0000-4000-8000-00000000a2f2 00000000-0000-4000-8000-00000000f001 --fields id,type,name
```

### entra principal diff

```text
agent-cli entra principal diff — Compare two users or identities: groups (nested too), directory and Azure roles
 *<left> str      One user, group or service principal: an object id, app id, UPN, name or @me
 *<right> str     The other, in the same forms
  --as str[]      Treat a word on the left as one on the right, A=B (repeatable); none turns off the names inferred from the two ids
  --ignore str[]  Leave out a path as the changes print it, and all under it (repeatable; * is one segment)
  --all           Keep what this resource's diff leaves out as noise (ids, times, counts)
  --limit int     Most differences to print (default 50)
  --exact         Texts compare as they are: whitespace, case and comments count
Returns: {left,right,same,as[],changes[{path,left,right,redacted,same,lines[],breaking}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra principal diff alice@contoso.example bob@contoso.example --fields same,changes
```

### entra user get

```text
agent-cli entra user get — Show an Entra user: UPN, enabled, manager, groups (nested) and directory roles
 *<user> str  The user: object id, UPN, mail (as ado person list prints it), display name, portal link, or @me
Returns: {id,name,upn,mail,type,enabled,created,synced,department,job_title,manager{id,name,upn},groups[{id,name}],roles[]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra user get sam@contoso.example --fields upn,enabled,groups,roles
```

### entra group list

```text
agent-cli entra group list — List Entra groups by name, or the groups a user or service principal is in
  <text> str    Only groups whose name starts with this
  --member str  Only the groups this user, group or service principal is in, nested ones too: an id, UPN, name or @me
  --owner str   Only the groups this user or service principal owns: an id, UPN, name, or @me (the az login)
  --limit int   (default 50)
Returns: [{id,name,kind,dynamic,direct}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli entra group list --member sam@contoso.example --fields id,name,direct
```

### entra group get

```text
agent-cli entra group get — Show an Entra group: kind, dynamic rule, owners, groups it is in, member counts
 *<group> str  The group: object id (arm role list prints it as principal_id), display name, or portal link
Returns: {id,name,description,kind,mail,dynamic_rule,rule_state,role_assignable,synced,created,owners[{id,name,upn,enabled}],member_of[{id,name}],members,transitive_members}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra group get sg-contoso-platform --fields kind,owners,members,transitive_members
```

### entra member list

```text
agent-cli entra member list — List who is in an Entra group, nested groups included, and through which
 *<group> str                   The group: object id (arm role list prints it as principal_id), display name, or portal link
  --direct                      Only its direct members: do not walk into nested groups
  --type user|group|serviceprincipal  Only members of this type
  --principal str               Only how this user, group or service principal is in it (every path): an id, UPN, name or @me
  --limit int                   (default 50)
Returns: [{id,type,name,upn,app_id,enabled,via[]}]
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra member list sg-contoso-platform --fields name,upn,via
```

### entra app list

```text
agent-cli entra app list — List Entra app registrations and when their credentials expire (no values)
  <text> str                 Only app registrations whose name starts with this
  --owner str                Only the apps this user or service principal owns: an id, UPN, name, or @me (the az login)
  --expires-within duration  Only apps with a secret or certificate ending within this long, or already expired
  --expired                  Only apps with a secret or certificate that has expired
  --limit int                (default 50)
A duration is 500ms, 30s, 15m, 2h, 7d, 1w.
Returns: [{id,app_id,name,created,next_expiry,credentials[{kind,key_id,name,starts,ends,expired}]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli entra app list --expires-within 30d --fields name,app_id,next_expiry
```

### entra app get

```text
agent-cli entra app get — Show an Entra app registration: credentials (no values), owners, federation
 *<app> str  The app registration: object id, app (client) id, display name, or portal link
Returns: {id,app_id,name,audience,created,service_principal,credentials[{kind,key_id,name,starts,ends,expired}],federated_credentials[{name,issuer,subject,serviceaccount}],owners[{id,name,upn,enabled}],permissions[{resource,name,kind}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra app get contoso-ado-deploy --fields app_id,credentials,owners
```

### entra serviceprincipal get

```text
agent-cli entra serviceprincipal get — Show an enterprise app or identity in Entra: the app roles and scopes granted it
 *<serviceprincipal> str  The service principal or managed identity: object id, client id (as arm identity get and k8s serviceaccount get print it), a managed identity's ARM id, display name, or portal link
Returns: {id,app_id,name,type,enabled,arm,home_tenant,assignment_required,credentials[{kind,key_id,name,starts,ends,expired}],app_roles[{resource,role}],delegated[{resource,scopes[],consent}],groups[{id,name}],owners[{id,name,upn,enabled}]}
Read. * required. Globals: --fields --raw --timeout --output
e.g. agent-cli entra serviceprincipal get 00000000-0000-4000-8000-00000000a1f1 --fields name,type,arm,app_roles,groups
```

### entra signin list

```text
agent-cli entra signin list — List Entra sign-ins of a user, service principal or managed identity
  --principal str            Whose: a user (object id, UPN, @me), or a service principal or managed identity (object id, client id, name), whose sign-ins only Graph beta lists; else users'
  --app str                  Only sign-ins to this app: its client id or display name
  --status failed|succeeded  Only failed or succeeded sign-ins
  --since time               From when (default a day before --until; P1 and P2 keep 30 days, Free 7)
  --until time               Until when (default now)
  --limit int                (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{when,principal,kind,app,resource,ip,error_code,error,key_id,ca_status,ca_policies[],correlation_id}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli entra signin list --principal contoso-ado-deploy --status failed --since 1d --fields when,error_code,error,key_id
```

### entra audit list

```text
agent-cli entra audit list — List the Entra audit log: who added or removed members, credentials, roles
  --target str               Only changes to this: an object id, or a user, group, app or service principal as its get takes it
  --caller str               Only changes this caller made: a UPN, @me, or an app's client id
  --activity str             Only this activity, exactly: Add member to group, Update application – Certificates and secrets management …
  --category str             Only this category: UserManagement, GroupManagement, ApplicationManagement, RoleManagement …
  --status failed|succeeded  Only changes that succeeded or failed
  --since time               From when (default 7 days before --until; P1 and P2 keep 30 days, Free 7)
  --until time               Until when (default now)
  --limit int                (default 50)
A time is 15m, 2h, 7d, 1w (ago), now-15m, 2026-09-29, or RFC 3339.
Returns: [{when,caller,activity,category,result,reason,targets[{id,type,name}],changes[{property,old,new}]}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli entra audit list --target sg-contoso-oncall --since 7d --fields when,caller,activity,targets
```

### entra role list

```text
agent-cli entra role list — List Entra directory roles held: who is Global Administrator, what X holds
  --principal str  Only the roles this user, group or service principal holds, through groups too: an id, UPN, name or @me
  --role str       Only who holds this role: its name (Global Administrator) or id
  --limit int      (default 50)
Returns: [{id,role,principal{id,type,name},scope,via}]
Read. Globals: --fields --raw --timeout --output
e.g. agent-cli entra role list --role "Global Administrator" --fields role,principal.name
```
