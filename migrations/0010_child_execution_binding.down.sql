DROP INDEX IF EXISTS ux_remote_cli_children_runtime_output;
DROP INDEX IF EXISTS ux_remote_cli_children_runtime_chat;
DROP INDEX IF EXISTS ux_remote_cli_children_parent_call;

ALTER TABLE remote_cli_children
    DROP CONSTRAINT IF EXISTS remote_cli_children_runtime_binding_complete,
    DROP CONSTRAINT IF EXISTS remote_cli_children_parent_binding_complete,
    DROP COLUMN IF EXISTS budget_settled,
    DROP COLUMN IF EXISTS budget_reserved_tokens,
    DROP COLUMN IF EXISTS budget_used_tokens,
    DROP COLUMN IF EXISTS child_output_id,
    DROP COLUMN IF EXISTS child_chat_session_id,
    DROP COLUMN IF EXISTS parent_call_id,
    DROP COLUMN IF EXISTS parent_output_id,
    DROP COLUMN IF EXISTS parent_chat_session_id;
