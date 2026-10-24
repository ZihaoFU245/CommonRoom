//! Current SQLite schema. Existing databases must match SCHEMA_VERSION.
pub(super) const SCHEMA_VERSION: u32 = 19;
pub(super) const SCHEMA: &str = "CREATE TABLE state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
CREATE TABLE read_positions (username TEXT NOT NULL, conversation TEXT NOT NULL, sequence INTEGER NOT NULL, PRIMARY KEY(username,conversation));
CREATE TABLE entities(kind TEXT NOT NULL,key TEXT NOT NULL,json TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE TABLE messages(conversation TEXT NOT NULL,sequence INTEGER NOT NULL,json TEXT NOT NULL,PRIMARY KEY(conversation,sequence)) WITHOUT ROWID;
CREATE TABLE reactions(conversation TEXT NOT NULL,sequence INTEGER NOT NULL,reaction TEXT NOT NULL,username TEXT NOT NULL,PRIMARY KEY(conversation,sequence,reaction,username),FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE TABLE counters(id INTEGER PRIMARY KEY CHECK(id=1),sequence INTEGER NOT NULL);
CREATE INDEX read_positions_conversation ON read_positions(conversation);";

pub(super) const RECEIPT_SCHEMA: &str = "CREATE TABLE bot_receipts(account_id TEXT NOT NULL,key TEXT NOT NULL,digest TEXT NOT NULL,conversation TEXT NOT NULL,sequence INTEGER NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(account_id,key)) WITHOUT ROWID;
CREATE INDEX bot_receipts_created ON bot_receipts(created);";

pub(super) const INDEXES: &str =
    "CREATE INDEX messages_id ON messages(json_extract(json,'$.id'),conversation);
CREATE INDEX messages_author ON messages(conversation,json_extract(json,'$.from'),sequence);
CREATE INDEX messages_time ON messages(conversation,json_extract(json,'$.time'));
CREATE INDEX messages_sequence ON messages(sequence);
CREATE INDEX entities_id ON entities(json_extract(json,'$.id')) WHERE kind IN ('room','private');";

pub(super) const AUDIT_SCHEMA: &str = "CREATE TABLE audit_entries(sequence INTEGER PRIMARY KEY AUTOINCREMENT,entry_id TEXT NOT NULL UNIQUE,json TEXT NOT NULL); CREATE INDEX messages_reply ON messages(conversation,json_extract(json,'$.reply.id'));";
// This reverse index covers author, recipient, reply author and mentions. SQL
// triggers keep it correct for all mutation paths, including reply scrubbing.
pub(super) const ACCOUNT_REFERENCE_SCHEMA: &str = "
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
";
