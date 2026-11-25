ALTER TABLE remote_cli_output_actions
    DROP CONSTRAINT remote_cli_output_actions_output_id_call_id_key;

ALTER TABLE remote_cli_output_actions
    ADD CONSTRAINT remote_cli_output_actions_session_id_output_id_call_id_key
    UNIQUE (session_id, output_id, call_id);
