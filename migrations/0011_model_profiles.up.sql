ALTER TABLE chats
    ADD COLUMN model_id text NOT NULL DEFAULT 'cortex-t0'
        CHECK (model_id IN ('cortex-t0', 'bonsai25-uncensored', 'bonsai25-official'));

ALTER TABLE chat_inputs
    ADD COLUMN model_id text NOT NULL DEFAULT 'cortex-t0'
        CHECK (model_id IN ('cortex-t0', 'bonsai25-uncensored', 'bonsai25-official'));
