# Identity, Authorization, And Security

## Decision

Every OwlMux installation is one self-hosted standalone deployment. It uses
exactly one configured human-authentication mode:

- **OwlAuth mode** for multiple authenticated users;
- **API-key mode** for one built-in owner authenticated by one deployment API
  key.

OwlMux owns organizations, memberships, roles, machine ownership, and machine
access. OwlAuth only authenticates users. Relay machine authentication and target
SSH authentication are separate credential classes.

PostgreSQL is durable authority. Redis is required disposable infrastructure for
bounded rate limits and cache. Neither owns tmux state.

## Principal Model

```text
AuthenticatedPrincipal {
    principal:
        owlauth_user(user_id, exact_project_issuer, subject) |
        api_key_owner,
    authentication_method:
        owlauth_session | deployment_api_key | api_key_session,
    session_id?,
    session_expires_at?,
    safe_presentation?,
}
```

An OwlAuth subject maps to one local OwlMux user only within the exact configured
Project issuer. Equal email, display name, avatar, provider identity, organization
claim, or custom claim never merges users or creates an OwlMux membership.

`api_key_owner` is a fixed built-in principal, not a mutable user row. API-key
mode creates one default organization and owner membership for it and admits no
other human users.

## OwlAuth Integration Contract

OwlAuth remains the authority for Project users, login transactions, Application
sessions, and Project access tokens. OwlMux is one OwlAuth Application inside one
configured Project.

The browser uses OwlAuth Project Auth, not downstream OIDC:

1. generic Project/Application login start;
2. hosted authentication through an admitted OwlAuth method;
3. exact allowlisted OwlMux redirect with one-use PKCE-bound handoff;
4. handoff exchange through the supported OwlAuth SDK;
5. one short-lived Project access token submitted to OwlMux session exchange.

OwlMux backend verification requires:

- an exact configured Project issuer, including its configured path and trailing
  slash semantics;
- exact immutable Project audience;
- exact configured OwlMux Application ID in `app_id`;
- JOSE `typ = at+jwt`;
- allowlisted `EdDSA`, current `kid`, and an Ed25519 Project JWKS key;
- valid signature, `iat`, `nbf`, `exp`, and bounded clock skew;
- stable Project-scoped `sub`;
- nonempty Application session `sid` and token `jti`;
- bounded token, claim, and presentation sizes.

OwlMux fetches JWKS only from the configured OwlAuth Project endpoint under
explicit HTTPS policy. An unknown `kid` can trigger one rate-limited refresh; the
current request fails and never accepts caller-selected key material.

A configured OwlAuth Project Server key may perform authoritative token
introspection at session exchange and before high-value access when current
revocation is required. It is backend-only and never becomes an OwlMux user,
browser, deployment API-key, organization, Relay, or SSH credential. Without
introspection, a locally valid token may remain accepted until its short `exp`.

OwlAuth authenticates. OwlMux authorizes. Provider roles, groups, organizations,
email domains, `belongs_to`, and custom claims do not bypass current local
organization membership.

## User Admission And Personal Organization

The first successful OwlAuth session exchange for an unknown `(issuer, sub)`
performs one PostgreSQL transaction that:

1. creates one active local user binding;
2. creates one `personal` organization with a stable ID and unique slug;
3. creates one active `owner` membership for that user;
4. records safe user presentation and source revision;
5. appends the admission audit event.

The transaction is idempotent under concurrent login. Equal presentation data
never selects an existing user. A user has exactly one personal organization.
The personal organization cannot lose its last owner or be transferred implicitly.

Safe presentation may update from a later authenticated OwlAuth projection, but
presentation changes never modify memberships or machine access.

In API-key mode, deployment initialization creates the equivalent fixed owner and
one default organization without an OwlAuth user row.

## Organization And Membership Model

An organization is the machine-sharing boundary inside one standalone deployment.
It is not a SaaS tenant, billing account, OwlAuth Project, or external identity
claim.

Initial roles are:

| Role     | Capabilities                                                                                      |
| -------- | ------------------------------------------------------------------------------------------------- |
| `owner`  | manage organization settings, owners, members, machines, enrollments, and all machine attachments |
| `admin`  | manage non-owner members, machines, enrollments, and all machine attachments                      |
| `member` | list and attach to every active machine in the organization                                       |

Capabilities are explicit authorization results. Role names never enter Relay or
SSH protocols.

An OwlAuth user may create shared organizations and may be an active member of
multiple organizations. Initial membership management adds an already admitted
local OwlAuth user by stable OwlMux user ID. Email invitation, domain auto-join,
SCIM, external group synchronization, and public join links are deferred.

Every active member can discover and connect to every active machine in that
organization. There is no per-machine ACL, owner-user field, viewer role, or
private machine inside a shared organization. A user who needs private machines
uses their personal organization.

Machine organization is immutable in the first release. Moving a machine requires
re-enrollment into a new machine record rather than rewriting credential and
audit scope.

Membership removal or disablement prevents new attachments and closes that
principal's existing OwlMux attachments within a bounded local propagation
interval. It never kills target tmux. Organization disablement closes all OwlMux
attachments and Relay route admission for its machines, again without target
process cleanup.

## Deployment API Key

API-key mode loads one high-entropy credential from protected deployment
configuration:

```text
OWLMUX_API_KEY=owlmux_sk_v1_<random>
```

Server never generates a production fallback, derives it from another secret,
accepts an empty or low-entropy value, or stores it in PostgreSQL or Redis. It
validates versioned prefix, decoded length, and canonical form and compares a
fixed domain-separated value in constant time.

The key is accepted only as:

- `Authorization: Bearer <key>` on OwlMux API requests; or
- a TLS-only same-origin browser-session exchange body.

It is forbidden in query strings, URLs, cookies, WebSocket subprotocols, logs,
telemetry, errors, audit, HTML, frontend bundles, or process arguments. There is
no online create, reveal, rotate, or recovery API. Replacing the environment value
invalidates all derived sessions and requires re-login.

API-key mode and OwlAuth mode are mutually exclusive. Changing mode is an
operator migration that invalidates all browser sessions; it is not a runtime
fallback from one credential verifier to another.

## Browser Session Security

OwlMux creates opaque high-entropy browser sessions after successful OwlAuth or
deployment API-key exchange. PostgreSQL stores only session digests and bounded
principal/authentication metadata. Redis may cache current validity but cannot
make an invalid PostgreSQL session valid.

Sessions are:

- rotated on authentication and mode change;
- bounded by idle and absolute expiry;
- no longer than the accepted OwlAuth token when created from OwlAuth;
- bound to authentication mode, principal, and captured credential generation;
- reauthorized against current user, organization, membership, and machine state;
- invalidated by logout, deployment-key replacement, OwlAuth user disablement when
  observed, or local user/membership disablement;
- transported only in `Secure`, `HttpOnly`, narrowly scoped cookies.

Cookie-authenticated state changes require CSRF protection and exact allowed
Origin. WebSocket upgrades additionally reject missing or cross-site Origin and
reauthorize organization membership and machine state.

## Machine Registration Credentials

An owner or admin creates an organization-owned pending machine. Server generates:

- one short-lived, one-use Relay enrollment token, stored only as a
  domain-separated PostgreSQL digest;
- one dedicated SSH key pair for Server-to-machine authentication;
- one encrypted SSH private-key envelope stored in PostgreSQL;
- safe public-key and expected-host-identity state.

The enrollment token is revealed once and is not an API key, user session, Relay
machine key, or SSH key. Relay generates and owns its own Ed25519 machine identity
during enrollment. Every credential class has a distinct prefix/schema, verifier,
accepted surface, audit field, and failure path.

A machine's SSH private key is recoverable product material because Server must
use it for later attachments. It therefore crosses only the secret-custody
interface. Relay and browser never receive it.

## Secret Custody

OwlMux provides one small object-safe compile-time interface for recoverable
secrets:

```text
SecretCustody {
    seal(context, plaintext) -> encrypted_envelope
    open(context, encrypted_envelope) -> plaintext
}
```

The context binds deployment, organization, machine, secret purpose, and envelope
schema. Implementations must authenticate that context so ciphertext cannot be
moved between machines or purposes.

The official binary statically composes one environment-root provider:

- `OWLMUX_SECRET_ROOT_KEY` contains one fixed high-entropy random key;
- the key is never generated silently, logged, stored in PostgreSQL/Redis, or
  returned by an API;
- per-record random nonces and versioned authenticated encryption protect each
  SSH private key;
- domain-separated key derivation and associated data bind the exact context;
- plaintext uses zeroizing wrappers and crosses the narrowest possible boundary.

There is no KMS SDK, remote-custody protocol, dynamic plugin loading, online key
creation, key version management, overlap, or rotation workflow in the initial
product. Operators that need KMS/HSM custody implement the interface and compile
their own server binary. The official server accepts exactly one root key; changing
it without an external offline migration makes existing encrypted material
unreadable and requires machine re-enrollment.

Non-recoverable values such as enrollment tokens and browser session IDs are
stored as domain-separated digests, not encrypted through secret custody.

## SSH Credential Boundary

Server runs OpenSSH under a dedicated unprivileged account and materializes one
machine private key only for the bounded lifetime of an SSH child through a
permission-restricted mechanism. It removes temporary material on normal and
abnormal child completion.

Server:

- uses an isolated Server-owned SSH configuration and strict enrolled host-key
  verification rather than the service account's ambient SSH config;
- selects only the registered per-machine target identity with `IdentitiesOnly`
  and disables the ambient agent for target authentication;
- disables password prompts, host-key prompts, agent forwarding, X11, local and
  remote forwarding, and interactive fallback;
- never sends SSH secrets to OwlAuth, browser, Redis, or Relay;
- never lets browser input choose identity paths or SSH options;
- redacts paths and diagnostics that expose credential layout.

Organization membership grants the selected target Unix account's effective
shell authority on every machine in that organization. Target OS permissions
remain authoritative. OwlMux does not isolate hostile members sharing that account
or tmux server.

## Relay Credential Boundary

Each Relay has a distinct machine key bound to exactly one organization-owned
machine and fixed loopback sshd endpoint. Relay authentication cannot authenticate
a user, create a browser session, satisfy SSH user authentication, or call an
OwlAuth API.

Relay private keys remain on the target. Revocation closes route admission but
never target tmux. A compromised Relay can relay or deny SSH streams for its own
machine, but cannot defeat SSH host-key or user-key verification without also
compromising target sshd or Server's protected SSH credential.

## PostgreSQL And Redis Security

PostgreSQL is authoritative for users, organizations, memberships, machines,
enrollment digests, Relay public keys, encrypted SSH material, browser-session
digests, and audit. Foreign keys and transactions enforce ownership and lifecycle
invariants.

Redis is required for bounded rate limits, negative/validity caches, and advisory
reachability only. Cache entries are namespaced and expire. Redis loss or flush
causes conservative cache misses and cold recovery; it cannot create a user,
membership, machine, session, or credential.

Neither store contains terminal input, terminal output, pane scrollback, tmux
projection, secret root keys, raw API keys, OwlAuth access/refresh tokens, Relay
private keys, or plaintext SSH private keys.

## Trust Model

### Server compromise

Server can observe live terminal input/output and decrypt registered machine SSH
credentials. Compromise therefore reaches every registered machine and can
impersonate the Web service. Secret custody protects at rest, not against a running
compromised Server.

### Target compromise

A compromised target controls sshd, tmux, shell, terminal output, and Relay.
Strict host-key verification detects routing to a different SSH host, not malicious
behavior by the expected target.

### OwlAuth compromise

A compromised configured OwlAuth Project can mint user tokens. Current OwlMux
organization memberships still determine machine access, but an attacker able to
mint a granted subject can assume that user.

### Browser compromise

A same-origin XSS can act through the current OwlMux session and observe terminal
data. Restrictive CSP, no third-party scripts, safe text rendering, and bounded
sessions are release requirements.

### Member trust

All members of one organization can access every organization machine as its
configured Unix account. Organizations must contain mutually trusted users.

## Command And Data Safety

- browser tmux operations are closed typed commands;
- names and IDs are validated before fixed rendering;
- pane input uses a literal byte path and is never command interpolation;
- SSH arguments are constructed without a local shell;
- remote startup uses one fixed tmux entry command;
- Relay destinations come only from enrolled machine state;
- terminal output and tmux presentation values are untrusted bytes/text;
- raw input and output are excluded from audit and telemetry;
- errors reveal organization and machine identity only after authorization.

## Resource And Denial-Of-Service Limits

Every HTTP, WebSocket, SSH child, tmux parser, and Relay boundary has explicit
body, frame, queue, concurrency, memory, and time limits. Redis-backed admission
limits protect login, user provisioning, organization mutation, machine creation,
enrollment, attachment creation, and Relay authentication.

High-cardinality user, organization, machine, session, pane, and stream IDs are
excluded from metrics labels. Overload rejects new work; it never terminates
target tmux.

## Audit

Safe audit events include:

- authentication exchange, user admission, and logout;
- organization and membership lifecycle;
- machine creation, enrollment, disablement, and re-enrollment;
- attachment start/end and route kind;
- SSH host verification and authentication reason class;
- Relay connect, disconnect, and revocation;
- destructive typed tmux operation intent and outcome class.

Audit excludes deployment API keys, OwlAuth tokens, cookies, Project Server keys,
secret root keys, Relay private keys/signatures, plaintext SSH private material,
raw pane input/output, and unsafe target diagnostics.

## Acceptance Criteria

- Exact OwlAuth issuer, audience, Application, JOSE type, EdDSA/JWKS, subject,
  session, and time checks have positive and negative fixtures.
- First OwlAuth admission transactionally creates exactly one user, personal
  organization, and owner membership under concurrent login.
- API-key mode creates exactly one built-in owner/default organization and never
  accepts OwlAuth credentials.
- Every active organization member can attach to every active organization
  machine; no nonmember can discover or attach to one.
- Membership revocation closes only OwlMux attachments and leaves target tmux.
- Enrollment tokens are one-use digests; SSH private keys are encrypted under
  exact machine/purpose context; root keys never enter storage.
- The official provider works with one fixed environment root and exposes no
  rotate API. A custom provider can be statically compiled against the interface.
- PostgreSQL loss blocks control decisions, Redis loss causes conservative cold
  recovery, and neither failure kills target tmux.
- SSH host-key mismatch fails closed through direct and Relay routes.
- No browser, Relay, OwlAuth, or deployment credential substitutes for another
  credential class.
