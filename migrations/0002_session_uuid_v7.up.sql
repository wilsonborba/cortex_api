ALTER TABLE chats ADD CONSTRAINT chats_session_uuid_v7 CHECK((get_byte(uuid_send(session_id),6) >> 4)=7);
