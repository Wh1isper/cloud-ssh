CREATE TABLE deployment (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    id uuid NOT NULL UNIQUE,
    default_ssh_credential_id uuid NOT NULL,
    config_epoch bigint NOT NULL CHECK (config_epoch > 0),
    server_build_id text NOT NULL CHECK (length(server_build_id) BETWEEN 1 AND 128),
    relay_protocol_version integer NOT NULL DEFAULT 1 CHECK (relay_protocol_version = 1),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE ssh_credentials (
    id uuid PRIMARY KEY,
    deployment_id uuid NOT NULL REFERENCES deployment(id) ON DELETE RESTRICT,
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 64 AND name = btrim(name)),
    public_key text NOT NULL CHECK (length(public_key) BETWEEN 40 AND 1024),
    public_fingerprint_sha256 text NOT NULL CHECK (length(public_fingerprint_sha256) BETWEEN 16 AND 128),
    encrypted_private_envelope bytea NOT NULL CHECK (octet_length(encrypted_private_envelope) BETWEEN 42 AND 16384),
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'retired')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE UNIQUE INDEX ssh_credentials_name_unique
    ON ssh_credentials (deployment_id, lower(name));

ALTER TABLE deployment
    ADD CONSTRAINT deployment_default_ssh_credential_fk
    FOREIGN KEY (default_ssh_credential_id)
    REFERENCES ssh_credentials(id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE machines (
    id uuid PRIMARY KEY,
    deployment_id uuid NOT NULL REFERENCES deployment(id) ON DELETE RESTRICT,
    ssh_credential_id uuid NOT NULL REFERENCES ssh_credentials(id) ON DELETE RESTRICT,
    alias text NOT NULL CHECK (length(alias) BETWEEN 1 AND 64 AND alias = btrim(alias)),
    lifecycle text NOT NULL DEFAULT 'pending' CHECK (lifecycle IN ('pending', 'verifying', 'active', 'disabled')),
    route_revision bigint NOT NULL DEFAULT 1 CHECK (route_revision > 0),
    credential_revision bigint NOT NULL DEFAULT 1 CHECK (credential_revision > 0),
    target_account text NOT NULL CHECK (target_account ~ '^[A-Za-z_][A-Za-z0-9_.-]{0,63}$'),
    tmux_path text NOT NULL CHECK (length(tmux_path) BETWEEN 1 AND 256 AND left(tmux_path, 1) = '/'),
    tmux_socket_identity text NOT NULL CHECK (length(tmux_socket_identity) BETWEEN 1 AND 128 AND tmux_socket_identity !~ '[[:cntrl:]]'),
    host_identity text NOT NULL CHECK (length(host_identity) BETWEEN 32 AND 2048 AND host_identity !~ '[[:cntrl:]]'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE UNIQUE INDEX machines_alias_unique ON machines (deployment_id, lower(alias));
CREATE INDEX machines_credential_idx ON machines (ssh_credential_id);

CREATE TABLE relay_enrollments (
    id uuid PRIMARY KEY,
    machine_id uuid NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    token_digest bytea NOT NULL UNIQUE CHECK (octet_length(token_digest) = 32),
    token_expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    cancelled_at timestamptz,
    status text NOT NULL CHECK (status IN ('issued', 'consumed', 'cancelled')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((status = 'issued' AND consumed_at IS NULL AND cancelled_at IS NULL)
        OR (status = 'consumed' AND consumed_at IS NOT NULL AND cancelled_at IS NULL)
        OR (status = 'cancelled' AND consumed_at IS NULL AND cancelled_at IS NOT NULL))
);

CREATE UNIQUE INDEX relay_enrollments_one_issued_per_machine
    ON relay_enrollments (machine_id) WHERE status = 'issued';

CREATE TABLE server_nodes (
    incarnation_id uuid PRIMARY KEY,
    display_name text CHECK (display_name IS NULL OR length(display_name) BETWEEN 1 AND 128),
    state text NOT NULL CHECK (state IN ('serving', 'draining')),
    config_epoch bigint NOT NULL CHECK (config_epoch > 0),
    server_build_id text NOT NULL CHECK (length(server_build_id) BETWEEN 1 AND 128),
    relay_protocol_version integer NOT NULL CHECK (relay_protocol_version = 1),
    lease_until timestamptz NOT NULL,
    registered_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    renewed_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX server_nodes_lease_idx ON server_nodes (lease_until);

CREATE TABLE audit_events (
    id uuid PRIMARY KEY,
    deployment_id uuid NOT NULL REFERENCES deployment(id) ON DELETE RESTRICT,
    resource_kind text NOT NULL CHECK (resource_kind IN ('deployment', 'ssh_credential', 'machine', 'enrollment', 'server_node')),
    machine_id uuid REFERENCES machines(id) ON DELETE RESTRICT,
    ssh_credential_id uuid REFERENCES ssh_credentials(id) ON DELETE RESTRICT,
    action text NOT NULL CHECK (length(action) BETWEEN 1 AND 64),
    outcome_class text NOT NULL CHECK (outcome_class IN ('success', 'rejected', 'ambiguous')),
    occurred_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((resource_kind = 'machine' AND machine_id IS NOT NULL)
        OR (resource_kind = 'enrollment' AND machine_id IS NOT NULL)
        OR (resource_kind = 'ssh_credential' AND ssh_credential_id IS NOT NULL)
        OR (resource_kind IN ('deployment', 'server_node')))
);

CREATE FUNCTION reject_ssh_credential_key_material_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.deployment_id <> OLD.deployment_id
        OR NEW.public_key <> OLD.public_key
        OR NEW.public_fingerprint_sha256 <> OLD.public_fingerprint_sha256
        OR NEW.encrypted_private_envelope <> OLD.encrypted_private_envelope
        OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'SSH credential key material is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER ssh_credential_key_material_immutable
BEFORE UPDATE ON ssh_credentials
FOR EACH ROW EXECUTE FUNCTION reject_ssh_credential_key_material_update();

CREATE FUNCTION reject_machine_scope_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.deployment_id <> OLD.deployment_id
        OR NEW.target_account <> OLD.target_account
        OR NEW.tmux_path <> OLD.tmux_path
        OR NEW.tmux_socket_identity <> OLD.tmux_socket_identity
        OR NEW.host_identity <> OLD.host_identity
        OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'Machine target scope is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER machine_target_scope_immutable
BEFORE UPDATE ON machines
FOR EACH ROW EXECUTE FUNCTION reject_machine_scope_update();
