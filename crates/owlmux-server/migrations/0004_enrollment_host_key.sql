ALTER TABLE machines
    ALTER COLUMN host_identity DROP NOT NULL,
    DROP CONSTRAINT machines_host_identity_check,
    ADD CONSTRAINT machines_host_identity_check
        CHECK (host_identity IS NULL
            OR (length(host_identity) BETWEEN 32 AND 2048 AND host_identity !~ '[[:cntrl:]]')),
    ADD CONSTRAINT machines_active_host_identity_check
        CHECK (lifecycle <> 'active' OR host_identity IS NOT NULL);

CREATE OR REPLACE FUNCTION reject_machine_scope_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.deployment_id <> OLD.deployment_id
        OR NEW.target_account <> OLD.target_account
        OR NEW.tmux_path <> OLD.tmux_path
        OR NEW.tmux_socket_identity <> OLD.tmux_socket_identity
        OR (NEW.host_identity IS DISTINCT FROM OLD.host_identity
            AND NOT (OLD.host_identity IS NULL
                AND NEW.host_identity IS NOT NULL
                AND OLD.lifecycle = 'verifying'
                AND NEW.lifecycle = 'verifying'))
        OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'Machine target scope is immutable';
    END IF;
    RETURN NEW;
END;
$$;
