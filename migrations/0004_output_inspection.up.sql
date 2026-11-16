CREATE TABLE output_inspections (
    output_id uuid PRIMARY KEY REFERENCES chat_outputs(output_id) ON DELETE CASCADE,
    model text NOT NULL,
    reason text CHECK (reason IN ('not_recorded', 'not_supported', 'invalid_data', 'incomplete'))
);
CREATE TABLE output_inspection_tokens (
    output_id uuid NOT NULL REFERENCES output_inspections(output_id) ON DELETE CASCADE,
    position integer NOT NULL CHECK (position >= 0),
    start_byte integer NOT NULL CHECK (start_byte >= 0),
    end_byte integer NOT NULL,
    bytes bytea NOT NULL CHECK (octet_length(bytes) <= 1024),
    logprob double precision NOT NULL CHECK (logprob <= 0 AND logprob > '-Infinity'::double precision),
    PRIMARY KEY (output_id, position),
    CHECK (end_byte = start_byte + octet_length(bytes))
);
CREATE TABLE output_inspection_alternatives (
    output_id uuid NOT NULL,
    token_position integer NOT NULL,
    rank smallint NOT NULL CHECK (rank BETWEEN 1 AND 5),
    bytes bytea NOT NULL CHECK (octet_length(bytes) <= 1024),
    logprob double precision NOT NULL CHECK (logprob <= 0 AND logprob > '-Infinity'::double precision),
    PRIMARY KEY (output_id, token_position, rank),
    FOREIGN KEY (output_id, token_position)
        REFERENCES output_inspection_tokens(output_id, position) ON DELETE CASCADE
);
