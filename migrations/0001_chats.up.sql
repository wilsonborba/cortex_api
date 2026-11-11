CREATE TABLE chats (
 session_id uuid PRIMARY KEY,
 app_id text NOT NULL,
 user_id text NOT NULL,
 title text NOT NULL,
 is_pinned boolean NOT NULL DEFAULT false,
 created_at timestamptz NOT NULL,
 updated_at timestamptz NOT NULL
);
CREATE TABLE chat_inputs (
 input_id uuid PRIMARY KEY,
 session_id uuid NOT NULL REFERENCES chats(session_id) ON DELETE CASCADE,
 text text NOT NULL,
 needs_web boolean NOT NULL,
 web_mode text NOT NULL,
 use_memory boolean NOT NULL,
 memory_topic text,
 thinking boolean NOT NULL,
 created_at timestamptz NOT NULL,
 UNIQUE(input_id,session_id)
);
CREATE TABLE chat_input_files (
 input_id uuid NOT NULL REFERENCES chat_inputs(input_id) ON DELETE CASCADE,
 file_id text NOT NULL,
 memory_id text NOT NULL,
 position integer NOT NULL CHECK(position>=0),
 PRIMARY KEY(input_id,position)
);
CREATE TABLE chat_outputs (
 output_id uuid PRIMARY KEY,
 input_id uuid NOT NULL UNIQUE,
 session_id uuid NOT NULL REFERENCES chats(session_id) ON DELETE CASCADE,
 status text NOT NULL CHECK(status IN ('running','done','cancelled','failed')),
 text text NOT NULL,
 thinking_text text,
 memory_id text,
 created_at timestamptz NOT NULL,
 finished_at timestamptz,
 FOREIGN KEY(input_id,session_id) REFERENCES chat_inputs(input_id,session_id) ON DELETE CASCADE
);
CREATE TABLE chat_output_sources (
 output_id uuid NOT NULL REFERENCES chat_outputs(output_id) ON DELETE CASCADE,
 position integer NOT NULL CHECK(position>=0),
 title text NOT NULL,
 url text NOT NULL,
 PRIMARY KEY(output_id,position)
);
CREATE TABLE chat_output_memories (
 output_id uuid NOT NULL REFERENCES chat_outputs(output_id) ON DELETE CASCADE,
 position integer NOT NULL CHECK(position>=0),
 memory_id text NOT NULL,
 title text NOT NULL,
 PRIMARY KEY(output_id,position)
);
CREATE INDEX ix_chats_user_app ON chats(user_id,app_id);
CREATE INDEX ix_chat_inputs_session_created ON chat_inputs(session_id,created_at);
CREATE INDEX ix_chat_outputs_session_created ON chat_outputs(session_id,created_at);
