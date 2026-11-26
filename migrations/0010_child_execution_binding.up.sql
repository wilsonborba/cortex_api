ALTER TABLE remote_cli_children
    ADD COLUMN parent_chat_session_id uuid,
    ADD COLUMN parent_output_id uuid,
    ADD COLUMN parent_call_id uuid,
    ADD COLUMN child_chat_session_id uuid REFERENCES chats(session_id) ON DELETE CASCADE,
    ADD COLUMN child_output_id uuid REFERENCES chat_outputs(output_id) ON DELETE CASCADE,
    ADD COLUMN budget_used_tokens bigint NOT NULL DEFAULT 0 CHECK (budget_used_tokens >= 0),
    ADD COLUMN budget_reserved_tokens bigint NOT NULL DEFAULT 0 CHECK (budget_reserved_tokens >= 0),
    ADD COLUMN budget_settled boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT remote_cli_children_parent_binding_complete CHECK (
        (parent_chat_session_id IS NULL AND parent_output_id IS NULL AND parent_call_id IS NULL)
        OR (parent_chat_session_id IS NOT NULL AND parent_output_id IS NOT NULL AND parent_call_id IS NOT NULL)
    ),
    ADD CONSTRAINT remote_cli_children_runtime_binding_complete CHECK (
        (child_chat_session_id IS NULL AND child_output_id IS NULL)
        OR (child_chat_session_id IS NOT NULL AND child_output_id IS NOT NULL)
    );

CREATE UNIQUE INDEX ux_remote_cli_children_parent_call
    ON remote_cli_children(parent_output_id, parent_call_id)
    WHERE parent_output_id IS NOT NULL;

CREATE UNIQUE INDEX ux_remote_cli_children_runtime_chat
    ON remote_cli_children(child_chat_session_id)
    WHERE child_chat_session_id IS NOT NULL;

CREATE UNIQUE INDEX ux_remote_cli_children_runtime_output
    ON remote_cli_children(child_output_id)
    WHERE child_output_id IS NOT NULL;
