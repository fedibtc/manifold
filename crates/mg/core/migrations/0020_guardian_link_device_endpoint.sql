-- The Iroh endpoint id the linked device called `link_device` from
-- (SPEC-guardian-link). Only that endpoint may open the host's web UI over
-- the host-UI ALPN. NULL for a link made before this column existed: such a
-- phone keeps its notifications and relinks to open the host UI.
ALTER TABLE guardian_link ADD COLUMN device_endpoint_id TEXT;
