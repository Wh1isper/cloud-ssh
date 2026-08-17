ALTER TABLE deployment
    ADD COLUMN profile text NOT NULL DEFAULT 'single_node',
    ADD COLUMN config_proof bytea;

ALTER TABLE deployment
    ADD CONSTRAINT deployment_profile_check
    CHECK (profile IN ('single_node', 'clustered')),
    ADD CONSTRAINT deployment_config_proof_check
    CHECK ((profile = 'single_node' AND config_proof IS NULL)
        OR (profile = 'clustered' AND octet_length(config_proof) = 32));

ALTER TABLE server_nodes
    ADD COLUMN internal_wss_url text;

ALTER TABLE server_nodes
    ADD CONSTRAINT server_nodes_internal_wss_url_check
    CHECK ((internal_wss_url IS NULL)
        OR (length(internal_wss_url) BETWEEN 16 AND 2048
            AND left(internal_wss_url, 6) = 'wss://'
            AND internal_wss_url !~ '[[:cntrl:]]'));
