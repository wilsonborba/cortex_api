CREATE TABLE remote_cli_children (
    session_id uuid NOT NULL REFERENCES remote_cli_sessions(session_id) ON DELETE CASCADE,
    child_id uuid NOT NULL,
    parent_run_id text NOT NULL CHECK (parent_run_id <> ''),
    role text NOT NULL CHECK (role <> '' AND length(role) <= 80),
    task text NOT NULL CHECK (task <> '' AND length(task) <= 32000),
    budget_tokens integer NOT NULL CHECK (budget_tokens > 0 AND budget_tokens <= 65536),
    depth smallint NOT NULL CHECK (depth > 0),
    status text NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    next_event_seq bigint NOT NULL DEFAULT 1 CHECK (next_event_seq > 0),
    cancel_requested_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (session_id, child_id)
);

CREATE INDEX ix_remote_cli_children_parent
    ON remote_cli_children(session_id, parent_run_id, created_at);

CREATE TABLE remote_cli_child_events (
    session_id uuid NOT NULL,
    child_id uuid NOT NULL,
    seq bigint NOT NULL CHECK (seq > 0),
    event_id uuid NOT NULL,
    kind text NOT NULL CHECK (kind IN (
        'queued', 'progress', 'completed', 'failed', 'cancel_requested', 'cancelled',
        'action_requested', 'action_answered'
    )),
    message text,
    result text,
    error_code text,
    action_id uuid,
    action_kind text,
    payload_digest text,
    display_text text,
    approved boolean,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (session_id, child_id, seq),
    UNIQUE (session_id, child_id, event_id),
    FOREIGN KEY (session_id, child_id)
        REFERENCES remote_cli_children(session_id, child_id) ON DELETE CASCADE
);

CREATE TABLE remote_cli_child_actions (
    session_id uuid NOT NULL,
    child_id uuid NOT NULL,
    action_id uuid NOT NULL,
    parent_run_id text NOT NULL CHECK (parent_run_id <> ''),
    action_kind text NOT NULL CHECK (action_kind IN ('file_change', 'command')),
    payload_digest text NOT NULL CHECK (payload_digest ~ '^[0-9a-f]{64}$'),
    display_text text NOT NULL CHECK (display_text <> '' AND length(display_text) <= 64000),
    expires_at timestamptz NOT NULL,
    approved boolean,
    answered_at timestamptz,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (session_id, child_id, action_id),
    FOREIGN KEY (session_id, child_id)
        REFERENCES remote_cli_children(session_id, child_id) ON DELETE CASCADE
);

CREATE INDEX ix_remote_cli_child_actions_expiry
    ON remote_cli_child_actions(expires_at)
    WHERE approved IS NULL;
