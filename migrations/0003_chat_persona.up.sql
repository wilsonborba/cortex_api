-- Area 23.5: the optional bot persona belongs to the chat.
ALTER TABLE chats ADD COLUMN persona_id text;
