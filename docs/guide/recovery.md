# Recovery and incident response

## Recovery boundary

OwlMux recovery restores reachability, not terminal ownership. Target sshd and target-owned tmux remain authoritative for sessions, PTYs, scrollback, layouts, and child processes. Server, Relay, Browser, PostgreSQL, or network failure may close OwlMux access but never authorizes OwlMux to kill, restart, recreate, or compensate target work.

PostgreSQL is the only durable OwlMux authority. The configured endpoint must expose one linearizable single-writer, non-rollback history and preserve acknowledged commits. OwlMux does not discover replicas, promote a writer, fence a former writer, validate a backup, or repair rolled-back history.

Run the repeatable recovery evidence with Docker:

```bash
make test-recovery
```

The single-node exercise proves PostgreSQL-loss hard fencing, credential rebind across an existing SSH child, use of the replacement credential by future children, and target tmux survival. The clustered exercise proves owner-process loss and lease recovery, unreachable-owner refusal, exact remote invalidation, cold API/configuration rotation, fresh process incarnations, and target tmux survival. These tests use disposable fixtures; they do not replace an operator's infrastructure-specific backup and restore rehearsal.

## Evidence to capture before an incident

Keep these materials in separate protected systems:

- PostgreSQL backups and restore instructions for the configured endpoint;
- the exact `OWLMUX_SSH_KEY_ENCRYPTION_KEY` matching encrypted credential envelopes;
- the current Deployment API key, cluster key, configuration epoch, public origin, and internal CA/node identities;
- the exact OwlMux release/build and schema compatibility record;
- target ownership, target public-key installation, and target-local tmux recovery procedures;
- reverse-proxy, load-balancer, and internal-network configuration.

Never place raw secrets in tickets, shell history, command-line arguments, logs, metrics, audit exports, or shared incident notes. Record only whether the required material was located and validated.

## Valid owner unreachable

`owner_unreachable` means PostgreSQL still names a lease-valid owner, but ingress cannot establish the authenticated internal owner WSS path. Do not bypass, clear, or steal that owner row.

1. Identify the named node from protected operator records and logs, not from Browser presentation.
2. Fence, stop, or network-isolate the whole node process or host.
3. Ensure the old process cannot resume from a suspended, cloned, or live-migrated snapshot.
4. Wait until PostgreSQL time observes its lease as expired.
5. Restore the internal TLS/network path or let the Relay reconnect through the public origin.
6. Retry from a fresh Browser attachment; expect a fresh target probe and chooser.

No live socket, pending input, writer pointer, or projection is transferred. Any target mutation already dispatched while the old owner was valid can still have a late result and is never replayed automatically.

## PostgreSQL endpoint outage

When the configured endpoint becomes unavailable, nodes become unready and irreversibly hard-fence no later than their local lease deadline. They close OwlMux Relay, attachment, SSH, and tmux control connections only. Target tmux continues.

1. Keep public readiness routing closed for fenced nodes.
2. Restore access to the same non-rolled-back writer history.
3. Confirm the database platform has fenced every former writer and preserves acknowledged commits.
4. Restart OwlMux nodes as fresh process incarnations; never resume old snapshots.
5. Let Relays reconnect and claim only after old leases are invalid.
6. Reopen Browser attachments through fresh authentication, owner resolution, target probe, and chooser.

Do not delete leases or owner rows to accelerate recovery. Database time and exact compare-and-set release are the authority.

## Backup restore or history replacement

A restore is a cold Deployment operation:

1. gate public and internal ingress;
2. stop or isolate every Server node and verify no process can resume;
3. stop Relays if they would create reconnect pressure during maintenance;
4. restore PostgreSQL using the operator-owned database procedure;
5. restore the exact matching SSH encryption key and coherent startup configuration;
6. ensure the restored database is the only active copy of that Deployment;
7. start only fresh Server process incarnations and verify `/ready` before reopening ingress;
8. let Relays reconnect and inspect Machines, credentials, owner epochs, and safe audit before allowing changes;
9. verify target tmux independently on each important target.

If the restored history lost any acknowledged commit or revives a consumed token, revoked Relay, former lease/owner epoch, old configuration, or retired credential state, OwlMux guarantees are unsupported. Treat this as a Deployment integrity incident, not normal failover. Rotate compromised authority, re-enroll Machines as needed, and reconcile target public keys externally. A database restore never restores Relay sockets, Browser memory, OpenSSH children, terminal output, writer state, or target tmux.

## Cold API-key or configuration rotation

API-key, cluster-key, Server-build, public-origin, encryption-key configuration, and other Deployment-critical changes do not roll online:

1. gate ingress, drain, and stop every Server node;
2. wait until PostgreSQL observes no valid old node lease;
3. prepare one coherent exact Server build and configuration on every intended node;
4. replace the sole API key or other intended setting and increment `OWLMUX_CONFIG_EPOCH` by exactly one;
5. retain the existing SSH encryption key unless executing a separately planned credential replacement incident response;
6. start nodes and require exact `/ready`, build, profile, proof, internal TLS, and protocol agreement;
7. verify the old API key receives `401` and the new key can read the expected Deployment epoch;
8. reopen the public origin.

There is no grace key, mixed-build rolling transition, per-node rotation, or old-page session. A Browser clears an invalid page-memory candidate and returns to `/login`. Target tmux continues throughout.

## SSH encryption-key loss or disclosure

If the encryption key is lost but trustworthy recovery material exists, keep nodes stopped, restore the exact value through the secret-management system, and start fresh nodes. Do not experiment with alternate keys against durable envelopes.

If the key cannot be recovered, stored private-key envelopes are unusable. Existing authenticated SSH children may end naturally, but no new child can authenticate from those credentials. Generate replacement credentials in a trustworthy Deployment, install their public keys through target administration, and explicitly rebind active Machines. Rebind affects future SSH children only and is not urgent revocation.

If the encryption key and database envelopes may both be disclosed:

1. disable affected Machines to close OwlMux access;
2. isolate compromised Server nodes and protect the database evidence;
3. generate replacements and install new public keys externally;
4. rebind/re-enroll only after the new authorization is present;
5. remove every exposed old public key from target authorization externally;
6. rotate Deployment and cluster authority through a cold configuration transition if node compromise is possible.

OwlMux never edits `authorized_keys` or kills target processes during this response.

## Node, cluster, Relay, or target compromise

- **Server node or cluster-key compromise:** treat the whole Deployment as exposed. Isolate all nodes, rotate API/cluster configuration cold, assess every stored SSH credential, replace target public keys where necessary, and start fresh incarnations.
- **Deployment API-key compromise:** gate access, cold-rotate the sole key and configuration epoch, inspect safe audit, and assess all Machine operations available to the holder.
- **Relay compromise:** revoke the Machine Relay, reset/re-enroll the fixed Machine only after the target is trusted, and inspect target authorization. Relay cannot manage tmux or authorization stores by design.
- **Target compromise:** disable the Machine, repair the target under target-administrator control, and rotate target authorization. Strict pin verification does not make a compromised pinned target safe. If repair changes the SSH host key, create a new Machine rather than re-enrolling the old pin.

## Post-incident checks

Before declaring recovery complete:

- every intended node is a fresh ready incarnation on one exact build/configuration;
- old keys, certificates, processes, database writers, and ingress paths are fenced;
- Machine lifecycle and advisory reachability are consistent with Relay state;
- credential reuse counts and target public-key installations match the intended blast radius;
- safe audit contains the expected durable lifecycle/configuration events and no credential or terminal content;
- protected metrics show bounded error/recovery classes without high-cardinality labels;
- target administrators independently confirm important tmux sessions and processes survived;
- no ambiguous terminal mutation was automatically retried or compensated.
