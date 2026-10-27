CREATE TABLE state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
CREATE TABLE read_positions (username TEXT NOT NULL, conversation TEXT NOT NULL, sequence INTEGER NOT NULL, PRIMARY KEY(username,conversation));
CREATE TABLE entities(kind TEXT NOT NULL,key TEXT NOT NULL,json TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE TABLE messages(conversation TEXT NOT NULL,sequence INTEGER NOT NULL,json TEXT NOT NULL,PRIMARY KEY(conversation,sequence)) WITHOUT ROWID;
CREATE TABLE reactions(conversation TEXT NOT NULL,sequence INTEGER NOT NULL,reaction TEXT NOT NULL,username TEXT NOT NULL,PRIMARY KEY(conversation,sequence,reaction,username),FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE TABLE counters(id INTEGER PRIMARY KEY CHECK(id=1),sequence INTEGER NOT NULL);
CREATE INDEX read_positions_conversation ON read_positions(conversation);
CREATE TABLE bot_receipts(account_id TEXT NOT NULL,key TEXT NOT NULL,digest TEXT NOT NULL,conversation TEXT NOT NULL,sequence INTEGER NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(account_id,key)) WITHOUT ROWID;
CREATE INDEX bot_receipts_created ON bot_receipts(created);
CREATE TABLE bot_command_receipts(account_id TEXT NOT NULL,key TEXT NOT NULL,digest TEXT NOT NULL,created INTEGER NOT NULL,outcome TEXT NOT NULL,PRIMARY KEY(account_id,key)) WITHOUT ROWID; CREATE INDEX bot_command_receipts_created ON bot_command_receipts(created);
CREATE TABLE audit_entries(sequence INTEGER PRIMARY KEY AUTOINCREMENT,entry_id TEXT NOT NULL UNIQUE,json TEXT NOT NULL); CREATE INDEX messages_reply ON messages(conversation,json_extract(json,'$.reply.id'));
CREATE INDEX messages_id ON messages(json_extract(json,'$.id'),conversation);
CREATE INDEX messages_author ON messages(conversation,json_extract(json,'$.from'),sequence);
CREATE INDEX messages_time ON messages(conversation,json_extract(json,'$.time'));
CREATE INDEX messages_sequence ON messages(sequence);
CREATE INDEX entities_id ON entities(json_extract(json,'$.id')) WHERE kind IN ('room','private');

CREATE TABLE message_accounts(name TEXT NOT NULL,conversation TEXT NOT NULL,sequence INTEGER NOT NULL,
 PRIMARY KEY(name,sequence,conversation), FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE ON UPDATE CASCADE) WITHOUT ROWID;
CREATE INDEX message_accounts_message ON message_accounts(conversation,sequence);
CREATE INDEX reactions_username ON reactions(username,conversation);
CREATE TRIGGER message_accounts_insert AFTER INSERT ON messages BEGIN
 INSERT OR IGNORE INTO message_accounts SELECT value,NEW.conversation,NEW.sequence FROM
 (SELECT json_extract(NEW.json,'$.from') AS value UNION SELECT json_extract(NEW.json,'$.to') UNION SELECT json_extract(NEW.json,'$.reply.from') UNION SELECT value FROM json_each(NEW.json,'$.mentions')) WHERE value IS NOT NULL;
END;
CREATE TRIGGER message_accounts_update AFTER UPDATE OF json ON messages BEGIN
 DELETE FROM message_accounts WHERE conversation=OLD.conversation AND sequence=OLD.sequence;
 INSERT OR IGNORE INTO message_accounts SELECT value,NEW.conversation,NEW.sequence FROM
 (SELECT json_extract(NEW.json,'$.from') AS value UNION SELECT json_extract(NEW.json,'$.to') UNION SELECT json_extract(NEW.json,'$.reply.from') UNION SELECT value FROM json_each(NEW.json,'$.mentions')) WHERE value IS NOT NULL;
END;
INSERT OR IGNORE INTO message_accounts SELECT json_extract(json,'$.from'),conversation,sequence FROM messages WHERE json_extract(json,'$.from') IS NOT NULL;
INSERT OR IGNORE INTO message_accounts SELECT json_extract(json,'$.to'),conversation,sequence FROM messages WHERE json_extract(json,'$.to') IS NOT NULL;
INSERT OR IGNORE INTO message_accounts SELECT json_extract(json,'$.reply.from'),conversation,sequence FROM messages WHERE json_extract(json,'$.reply.from') IS NOT NULL;
INSERT OR IGNORE INTO message_accounts SELECT j.value,m.conversation,m.sequence FROM messages m,json_each(m.json,'$.mentions') j;


CREATE TABLE IF NOT EXISTS media(id TEXT PRIMARY KEY, uploader TEXT NOT NULL, conversation TEXT NOT NULL, size INTEGER NOT NULL, created INTEGER NOT NULL, ready INTEGER NOT NULL, unreferenced INTEGER, staging_clean INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL, expires INTEGER, published INTEGER, deleting INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, cleanup_attempt INTEGER NOT NULL DEFAULT 0, staging_attempt INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS message_media(conversation TEXT NOT NULL, sequence INTEGER NOT NULL, media_id TEXT NOT NULL REFERENCES media(id), PRIMARY KEY(conversation,sequence,media_id), FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS message_media_id ON message_media(media_id);


CREATE INDEX IF NOT EXISTS media_quota ON media(uploader,deleted,published,created,size);
CREATE INDEX IF NOT EXISTS media_due_retry ON media(cleanup_attempt,created) WHERE deleted=0 AND deleting=1;
CREATE INDEX IF NOT EXISTS media_due_expiry ON media(expires,created) WHERE deleted=0 AND deleting=0;
CREATE INDEX IF NOT EXISTS media_due_orphan ON media(unreferenced,created) WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL;
CREATE INDEX IF NOT EXISTS media_staging_due ON media(staging_attempt,created) WHERE staging_clean=0;
CREATE INDEX IF NOT EXISTS media_tombstone ON media(created) WHERE deleted=1 AND staging_clean=1;
CREATE TRIGGER IF NOT EXISTS media_reference_removed AFTER DELETE ON message_media BEGIN
 UPDATE media SET unreferenced=COALESCE(unreferenced,unixepoch()) WHERE id=OLD.media_id AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=OLD.media_id);
END;


CREATE TABLE retention(conversation TEXT PRIMARY KEY, count INTEGER NOT NULL CHECK(count>=0), oldest INTEGER) WITHOUT ROWID;
CREATE TRIGGER retention_insert AFTER INSERT ON messages BEGIN
 INSERT INTO retention VALUES(NEW.conversation,1,NEW.sequence) ON CONFLICT(conversation) DO UPDATE SET count=count+1,oldest=min(coalesce(oldest,NEW.sequence),NEW.sequence);
END;
CREATE TRIGGER retention_delete AFTER DELETE ON messages BEGIN
 UPDATE retention SET count=count-1,oldest=(SELECT min(sequence) FROM messages WHERE conversation=OLD.conversation) WHERE conversation=OLD.conversation;
 DELETE FROM retention WHERE conversation=OLD.conversation AND count=0;
END;
CREATE TRIGGER retention_move AFTER UPDATE OF conversation,sequence ON messages WHEN OLD.conversation!=NEW.conversation OR OLD.sequence!=NEW.sequence BEGIN
 DELETE FROM retention WHERE conversation IN (OLD.conversation,NEW.conversation);
 INSERT INTO retention SELECT conversation,count(*),min(sequence) FROM messages WHERE conversation IN (OLD.conversation,NEW.conversation) GROUP BY conversation;
END;

CREATE TABLE auth_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,at INTEGER NOT NULL,client TEXT,username TEXT,outcome TEXT NOT NULL CHECK(outcome IN ('success','failure','logout')));
PRAGMA user_version=19;
