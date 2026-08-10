# Security

## Current foundation

The current Server exposes only placeholder static assets plus `/health` and
`/ready`. API and auth prefixes return explicit not-implemented responses. Relay
makes no network connection. No product credentials or terminal data exist yet.

## Target trust boundary

Once implemented, OwlMux Server will be a high-trust bastion:

- it can observe live terminal input and output;
- it can decrypt per-machine SSH credentials while attaching;
- it authorizes organization members to target Unix accounts;
- compromise reaches registered machines available to those credentials.

Secret custody protects database contents at rest. It does not protect against a
running compromised Server.

## Target and member trust

Target tmux and sshd are authoritative. A compromised expected target can emit
malicious terminal data or control its shell. Strict host-key verification
prevents silent routing to a different host; it does not make the expected host
trustworthy.

Every active organization member can access every organization machine as its
configured Unix account. Organizations must contain mutually trusted members.
OwlMux does not provide hostile-user isolation inside one Unix account or tmux
server.

## Credential separation

The target design keeps these credentials distinct:

- OwlAuth Project access token;
- deployment API key;
- OwlMux browser session;
- Relay enrollment token;
- Relay machine key;
- per-machine SSH key;
- OwlAuth Project Server key;
- secret-custody root key.

No credential is accepted as another class. Raw values remain out of URLs, logs,
telemetry, audit, and browser storage. The target design invokes OpenSSH through a
dedicated Server-owned configuration with strict host-key inputs, the exact
per-machine target key, `IdentitiesOnly`, and no ambient target-authentication
agent. Any later bastion profile is separately operator configured.

## Process continuity

Security revocation closes OwlMux access and Relay route admission. It does not
kill target tmux. Destroying a tmux session is an explicit typed user operation,
not an authorization or infrastructure cleanup side effect.

## Reporting vulnerabilities

Report security issues privately through the repository's
[security policy](https://github.com/owlfoundry/owlmux/security/policy). Do not
open a public issue for an undisclosed vulnerability.

The normative threat model and acceptance criteria are in the
[security specification](https://github.com/owlfoundry/owlmux/blob/main/spec/06-identity-authorization-and-security.md).
