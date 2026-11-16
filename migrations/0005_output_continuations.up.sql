CREATE TABLE output_continuations (
    output_id uuid PRIMARY KEY REFERENCES chat_outputs(output_id) ON DELETE CASCADE,
    source_output_id uuid NOT NULL REFERENCES chat_outputs(output_id) ON DELETE CASCADE,
    token_position integer NOT NULL CHECK (token_position >= 0),
    alternative_rank smallint NOT NULL CHECK (alternative_rank BETWEEN 1 AND 5),
    source_byte integer NOT NULL CHECK (source_byte >= 0),
    CHECK (output_id <> source_output_id)
);
CREATE INDEX output_continuations_source ON output_continuations(source_output_id);
