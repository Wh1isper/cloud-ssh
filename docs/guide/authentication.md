# Authentication and organizations

::: warning Target design
Authentication, users, organizations, memberships, and machines are specified but
not implemented in the current foundation.
:::

## Two exclusive authentication modes

Every self-hosted deployment chooses one mode.

### OwlAuth mode

OwlMux integrates with OwlAuth Project Auth rather than treating OwlAuth as a
downstream OIDC provider. The browser completes OwlAuth Hosted login and PKCE
handoff, then exchanges a short-lived Project access token for an opaque OwlMux
session.

Server validates the exact Project issuer and audience, OwlMux Application ID,
JOSE type, EdDSA/JWKS signature, subject, Application session, and time claims.
OwlAuth authenticates the user; OwlMux still owns product authorization.

### API-key mode

Without OwlAuth, one high-entropy `OWLMUX_API_KEY` authenticates a built-in owner.
The browser exchanges it once over TLS for an HTTP-only OwlMux session. The raw
key is never retained in browser storage or application persistence.

The two modes are not fallback verifiers. A deployment exposes only the configured
exchange path.

## Personal and shared organizations

On first OwlAuth admission, one transaction creates:

- the local OwlMux user binding;
- one personal organization;
- one owner membership.

Users can create shared organizations and add already admitted OwlMux users.
Initial roles are owner, admin, and member. Every active member can discover and
attach to every active machine in that organization. There is deliberately no
per-machine ACL; private machines belong in the user's personal organization.

API-key mode has one built-in owner and one default organization.

## Machine registration

An owner or admin creates an organization-owned pending machine. Server generates
one per-machine SSH key and one short-lived Relay enrollment token. Relay consumes
the token once, binds its machine key and target SSH host identity, and installs
or verifies the generated public key for the selected local account.

The SSH private key remains encrypted in PostgreSQL through the Server secret
custody boundary. Browser and Relay never receive it.

## Secret custody

The official Server will read one fixed `OWLMUX_SECRET_ROOT_KEY` environment
value. It will not expose online key creation, rotation, KMS management, or
multiple-root fallback.

A small compile-time interface will let operators implement KMS/HSM custody and
build their own Server. Non-recoverable values such as browser sessions and
enrollment tokens remain digests rather than encrypted secrets.

Read the normative [identity and security specification](https://github.com/owlfoundry/owlmux/blob/main/spec/06-identity-authorization-and-security.md).
