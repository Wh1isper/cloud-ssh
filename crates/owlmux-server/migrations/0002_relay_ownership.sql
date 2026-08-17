CREATE TABLE relay_verification_attempts (
    id uuid PRIMARY KEY,
    machine_id uuid NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    enrollment_id uuid NOT NULL UNIQUE REFERENCES relay_enrollments(id) ON DELETE RESTRICT,
    executing_incarnation_id uuid REFERENCES server_nodes(incarnation_id) ON DELETE SET NULL,
    route_revision bigint NOT NULL CHECK (route_revision > 0),
    status text NOT NULL CHECK (status IN ('verifying', 'activated', 'failed')),
    deadline timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    completed_at timestamptz,
    CHECK ((status = 'verifying' AND completed_at IS NULL AND executing_incarnation_id IS NOT NULL)
        OR (status IN ('activated', 'failed') AND completed_at IS NOT NULL))
);

CREATE UNIQUE INDEX relay_verification_one_live_per_machine
    ON relay_verification_attempts (machine_id) WHERE status = 'verifying';

CREATE TABLE relay_bindings (
    id uuid PRIMARY KEY,
    machine_id uuid NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    relay_id uuid NOT NULL,
    relay_public_key bytea NOT NULL CHECK (octet_length(relay_public_key) = 32),
    route_revision bigint NOT NULL CHECK (route_revision > 0),
    status text NOT NULL CHECK (status IN ('active', 'revoked')),
    activated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    revoked_at timestamptz,
    CHECK ((status = 'active' AND revoked_at IS NULL)
        OR (status = 'revoked' AND revoked_at IS NOT NULL))
);

CREATE UNIQUE INDEX relay_bindings_one_active_per_machine
    ON relay_bindings (machine_id) WHERE status = 'active';
CREATE UNIQUE INDEX relay_bindings_active_relay_id_unique
    ON relay_bindings (relay_id) WHERE status = 'active';
CREATE UNIQUE INDEX relay_bindings_active_public_key_unique
    ON relay_bindings (relay_public_key) WHERE status = 'active';

CREATE TABLE machine_owners (
    machine_id uuid PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    connection_epoch bigint NOT NULL DEFAULT 0 CHECK (connection_epoch >= 0),
    owner_incarnation_id uuid REFERENCES server_nodes(incarnation_id) ON DELETE RESTRICT,
    relay_connection_id uuid,
    route_revision bigint NOT NULL CHECK (route_revision > 0),
    claimed_at timestamptz,
    CHECK ((owner_incarnation_id IS NULL AND relay_connection_id IS NULL AND claimed_at IS NULL)
        OR (owner_incarnation_id IS NOT NULL AND relay_connection_id IS NOT NULL AND claimed_at IS NOT NULL))
);

INSERT INTO machine_owners (machine_id, route_revision)
SELECT id, route_revision FROM machines;

ALTER TABLE audit_events DROP CONSTRAINT audit_events_resource_kind_check;
ALTER TABLE audit_events ADD CONSTRAINT audit_events_resource_kind_check
    CHECK (resource_kind IN ('deployment', 'ssh_credential', 'machine', 'enrollment', 'relay_binding', 'machine_owner', 'server_node'));

ALTER TABLE audit_events DROP CONSTRAINT audit_events_check;
ALTER TABLE audit_events ADD CONSTRAINT audit_events_resource_reference_check
    CHECK ((resource_kind = 'machine' AND machine_id IS NOT NULL)
        OR (resource_kind = 'enrollment' AND machine_id IS NOT NULL)
        OR (resource_kind = 'relay_binding' AND machine_id IS NOT NULL)
        OR (resource_kind = 'machine_owner' AND machine_id IS NOT NULL)
        OR (resource_kind = 'ssh_credential' AND ssh_credential_id IS NOT NULL)
        OR (resource_kind IN ('deployment', 'server_node')));
