DO $migration$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM remote_cli_output_actions
        GROUP BY output_id, call_id
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate output/call approvals require manual reconciliation before single-executor migration';
    END IF;
END
$migration$;

ALTER TABLE remote_cli_output_actions
    DROP CONSTRAINT remote_cli_output_actions_session_id_output_id_call_id_key;

ALTER TABLE remote_cli_output_actions
    ADD CONSTRAINT remote_cli_output_actions_output_id_call_id_key
    UNIQUE (output_id, call_id);
