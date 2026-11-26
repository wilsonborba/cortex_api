CREATE TABLE remote_cli_sessions (
    session_id uuid PRIMARY KEY,
    owner_user text NOT NULL CHECK (owner_user <> ''),
    device_name text NOT NULL CHECK (device_name <> ''),
    platform text NOT NULL CHECK (platform <> ''),
    cli_connected boolean NOT NULL DEFAULT true,
    last_seen_at timestamptz NOT NULL,
    last_disconnect_at timestamptz,
    next_cli_seq bigint NOT NULL DEFAULT 0 CHECK (next_cli_seq >= 0),
    next_ctrl_seq bigint NOT NULL DEFAULT 0 CHECK (next_ctrl_seq >= 0),
    created_at timestamptz NOT NULL
);

CREATE INDEX ix_remote_cli_sessions_owner ON remote_cli_sessions(owner_user, last_seen_at DESC);
CREATE INDEX ix_remote_cli_sessions_last_seen ON remote_cli_sessions(last_seen_at);

CREATE TABLE remote_cli_commands (
    session_id uuid NOT NULL REFERENCES remote_cli_sessions(session_id) ON DELETE CASCADE,
    seq bigint NOT NULL CHECK (seq > 0),
    kind text NOT NULL CHECK (kind IN ('prompt', 'approval_answer', 'cancel')),
    prompt_text text,
    turn_id text,
    approval_id text,
    approved boolean,
    ttl_ms bigint NOT NULL CHECK (ttl_ms > 0),
    acked_at timestamptz,
    expired_reported_at timestamptz,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (session_id, seq)
);

CREATE INDEX ix_remote_cli_commands_pending ON remote_cli_commands(created_at)
    WHERE acked_at IS NULL AND expired_reported_at IS NULL;

CREATE TABLE remote_cli_command_files (
    session_id uuid NOT NULL,
    seq bigint NOT NULL,
    position integer NOT NULL CHECK (position >= 0),
    file_id text NOT NULL CHECK (file_id <> ''),
    PRIMARY KEY (session_id, seq, position),
    FOREIGN KEY (session_id, seq) REFERENCES remote_cli_commands(session_id, seq) ON DELETE CASCADE
);

CREATE TABLE remote_cli_events (
    session_id uuid NOT NULL REFERENCES remote_cli_sessions(session_id) ON DELETE CASCADE,
    seq bigint NOT NULL CHECK (seq > 0),
    kind text NOT NULL CHECK (kind IN (
        'progress', 'answer_part', 'approval_request', 'approval_answer',
        'artifact', 'turn_end', 'cancel', 'command_expired'
    )),
    turn_id text,
    message_text text,
    stage text,
    approval_id text,
    action_text text,
    approved boolean,
    file_id text,
    artifact_name text,
    content_type text,
    command_id bigint,
    status text,
    created_at timestamptz NOT NULL,
    PRIMARY KEY (session_id, seq)
);

CREATE TABLE remote_cli_approvals (
    session_id uuid NOT NULL REFERENCES remote_cli_sessions(session_id) ON DELETE CASCADE,
    approval_id text NOT NULL CHECK (approval_id <> ''),
    approved boolean NOT NULL,
    answered_at timestamptz NOT NULL,
    PRIMARY KEY (session_id, approval_id)
);
