use balabol_crypto::ContactCard;
use loro::{LoroDoc, LoroList, LoroValue, ToJson, VersionVector};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("SQLite database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Loro CRDT error: {0}")]
    Loro(#[from] loro::LoroError),
    #[error("Loro encode error: {0}")]
    LoroEncode(#[from] loro::LoroEncodeError),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Channel not found: {0}")]
    ChannelNotFound(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub channel_id: String,
    pub author_id: String,
    pub content: String,
    pub attachments: Vec<String>,
    pub timestamp_ms: i64,
    pub signature_hex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRoom {
    pub id: String,
    pub name: String,
    pub emoji: String,   // e.g. "🎮" shown in the icon sidebar
    pub invite_code: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRoomMember {
    pub room_id: String,
    pub user_id: String,
    pub role: String,
    pub joined_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredChannel {
    pub id: String,
    pub server_id: Option<String>,
    pub name: String,
    pub channel_type: String, // "text" | "voice" | "video"
    pub topic: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAudioPreference {
    pub user_id: String,
    pub volume: f32, // 0.0 to 2.0 (1.0 = 100%)
    pub is_muted: bool,
}

/// SQLite local storage for fast queries, offline cache, and contact book.
pub struct LocalDatabase {
    conn: Mutex<Connection>,
}

impl LocalDatabase {
    /// Open or create database at specified path, or in memory if path is empty/":memory:".
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_tables()?;
        Ok(db)
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_tables()?;
        Ok(db)
    }

    fn init_tables(&self) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS contacts (
                user_id TEXT PRIMARY KEY,
                public_key_hex TEXT NOT NULL,
                display_name TEXT NOT NULL,
                avatar_url TEXT,
                bio TEXT,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS rooms (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                emoji TEXT NOT NULL DEFAULT '🎮',
                invite_code TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS room_members (
                room_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'member',
                joined_at INTEGER NOT NULL,
                PRIMARY KEY(room_id, user_id)
            );

            CREATE TABLE IF NOT EXISTS channels (
                id TEXT PRIMARY KEY,
                server_id TEXT,
                name TEXT NOT NULL,
                channel_type TEXT NOT NULL,
                topic TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                channel_id TEXT NOT NULL,
                author_id TEXT NOT NULL,
                content TEXT NOT NULL,
                attachments_json TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                signature_hex TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel_id, timestamp_ms);

            CREATE TABLE IF NOT EXISTS user_audio_settings (
                user_id TEXT PRIMARY KEY,
                volume REAL NOT NULL DEFAULT 1.0,
                is_muted INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS crdt_snapshots (
                doc_id TEXT PRIMARY KEY,
                snapshot_bytes BLOB NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(())
    }

    // --- Contacts & Profiles ---
    pub fn save_contact(&self, contact: &ContactCard) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        // If saving a permanent 'bala:' contact with endpoint in bio, clean up any temporary 'peer:' stub with the same endpoint
        if contact.user_id.starts_with("bala:") {
            if let Some(ref ep) = contact.bio {
                let _ = conn.execute("DELETE FROM contacts WHERE user_id LIKE 'peer:%' AND (bio = ?1 OR user_id = ?2)", params![ep, format!("peer:{}", ep.replace(':', "_"))]);
            }
        } else if contact.user_id.starts_with("peer:") {
            // If a 'bala:' contact with the same endpoint already exists, do not re-add a 'peer:' stub
            if let Some(ref ep) = contact.bio {
                let exists: bool = conn.query_row(
                    "SELECT 1 FROM contacts WHERE user_id LIKE 'bala:%' AND bio = ?1 LIMIT 1",
                    params![ep],
                    |_| Ok(true),
                ).unwrap_or(false);
                if exists {
                    return Ok(());
                }
            }
        }

        conn.execute(
            r#"
            INSERT INTO contacts (user_id, public_key_hex, display_name, avatar_url, bio, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(user_id) DO UPDATE SET
                display_name=excluded.display_name,
                avatar_url=excluded.avatar_url,
                bio=excluded.bio,
                updated_at=excluded.updated_at
            "#,
            params![
                contact.user_id,
                contact.public_key_hex,
                contact.display_name,
                contact.avatar_url,
                contact.bio,
                now
            ],
        )?;
        Ok(())
    }

    pub fn get_contact(&self, user_id: &str) -> Result<Option<ContactCard>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT user_id, public_key_hex, display_name, avatar_url, bio FROM contacts WHERE user_id = ?1",
        )?;
        let result = stmt
            .query_row(params![user_id], |row| {
                Ok(ContactCard {
                    user_id: row.get(0)?,
                    public_key_hex: row.get(1)?,
                    display_name: row.get(2)?,
                    avatar_url: row.get(3)?,
                    bio: row.get(4)?,
                })
            })
            .optional()?;
        Ok(result)
    }

    pub fn list_contacts(&self) -> Result<Vec<ContactCard>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT user_id, public_key_hex, display_name, avatar_url, bio FROM contacts ORDER BY display_name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ContactCard {
                user_id: row.get(0)?,
                public_key_hex: row.get(1)?,
                display_name: row.get(2)?,
                avatar_url: row.get(3)?,
                bio: row.get(4)?,
            })
        })?;

        let mut contacts = Vec::new();
        for r in rows {
            contacts.push(r?);
        }
        Ok(contacts)
    }

    pub fn delete_contact(&self, user_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM contacts WHERE user_id = ?1", params![user_id])?;
        Ok(())
    }

    // --- Rooms & Room Members ---
    pub fn save_room(&self, room: &StoredRoom) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO rooms (id, name, emoji, invite_code, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                name=excluded.name,
                emoji=excluded.emoji,
                invite_code=excluded.invite_code
            "#,
            params![
                room.id,
                room.name,
                room.emoji,
                room.invite_code,
                room.created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_rooms(&self) -> Result<Vec<StoredRoom>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, emoji, invite_code, created_at FROM rooms ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredRoom {
                id: row.get(0)?,
                name: row.get(1)?,
                emoji: row.get(2)?,
                invite_code: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        let mut rooms = Vec::new();
        for r in rows {
            rooms.push(r?);
        }
        Ok(rooms)
    }

    pub fn delete_room(&self, id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        // Delete members, channels, and room record
        conn.execute("DELETE FROM room_members WHERE room_id = ?1", params![id])?;
        conn.execute("DELETE FROM channels WHERE server_id = ?1", params![id])?;
        conn.execute("DELETE FROM rooms WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn add_room_member(&self, room_id: &str, user_id: &str, role: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        conn.execute(
            r#"
            INSERT INTO room_members (room_id, user_id, role, joined_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(room_id, user_id) DO UPDATE SET role=excluded.role
            "#,
            params![room_id, user_id, role, now],
        )?;
        Ok(())
    }

    pub fn remove_room_member(&self, room_id: &str, user_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM room_members WHERE room_id = ?1 AND user_id = ?2",
            params![room_id, user_id],
        )?;
        Ok(())
    }

    pub fn list_room_members(&self, room_id: &str) -> Result<Vec<StoredRoomMember>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT room_id, user_id, role, joined_at FROM room_members WHERE room_id = ?1 ORDER BY joined_at ASC",
        )?;
        let rows = stmt.query_map(params![room_id], |row| {
            Ok(StoredRoomMember {
                room_id: row.get(0)?,
                user_id: row.get(1)?,
                role: row.get(2)?,
                joined_at: row.get(3)?,
            })
        })?;
        let mut members = Vec::new();
        for r in rows {
            members.push(r?);
        }
        Ok(members)
    }

    // --- Channels ---
    pub fn save_channel(&self, channel: &StoredChannel) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO channels (id, server_id, name, channel_type, topic, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(id) DO UPDATE SET
                name=excluded.name,
                topic=excluded.topic
            "#,
            params![
                channel.id,
                channel.server_id,
                channel.name,
                channel.channel_type,
                channel.topic,
                channel.created_at
            ],
        )?;
        Ok(())
    }

    pub fn list_channels(&self, server_id: Option<&str>) -> Result<Vec<StoredChannel>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = match server_id {
            Some(_) => conn.prepare("SELECT id, server_id, name, channel_type, topic, created_at FROM channels WHERE server_id = ?1 ORDER BY created_at ASC")?,
            None => conn.prepare("SELECT id, server_id, name, channel_type, topic, created_at FROM channels ORDER BY created_at ASC")?,
        };

        let map_row = |row: &rusqlite::Row| {
            Ok(StoredChannel {
                id: row.get(0)?,
                server_id: row.get(1)?,
                name: row.get(2)?,
                channel_type: row.get(3)?,
                topic: row.get(4)?,
                created_at: row.get(5)?,
            })
        };

        let rows = if let Some(sid) = server_id {
            stmt.query_map(params![sid], map_row)?
        } else {
            stmt.query_map([], map_row)?
        };

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn get_channel(&self, id: &str) -> Result<Option<StoredChannel>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, server_id, name, channel_type, topic, created_at FROM channels WHERE id = ?1",
        )?;
        let result = stmt
            .query_row(params![id], |row| {
                Ok(StoredChannel {
                    id: row.get(0)?,
                    server_id: row.get(1)?,
                    name: row.get(2)?,
                    channel_type: row.get(3)?,
                    topic: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })
            .optional()?;
        Ok(result)
    }

    pub fn delete_channel(&self, id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM messages WHERE channel_id = ?1", params![id])?;
        conn.execute("DELETE FROM channels WHERE id = ?1", params![id])?;
        Ok(())
    }

    // --- Messages Cache ---
    pub fn save_message(&self, msg: &StoredMessage) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let attachments_json = serde_json::to_string(&msg.attachments)?;
        conn.execute(
            r#"
            INSERT OR REPLACE INTO messages (id, channel_id, author_id, content, attachments_json, timestamp_ms, signature_hex)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                msg.id,
                msg.channel_id,
                msg.author_id,
                msg.content,
                attachments_json,
                msg.timestamp_ms,
                msg.signature_hex
            ],
        )?;
        Ok(())
    }

    pub fn get_messages(&self, channel_id: &str, limit: usize) -> Result<Vec<StoredMessage>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, channel_id, author_id, content, attachments_json, timestamp_ms, signature_hex
            FROM messages
            WHERE channel_id = ?1
            ORDER BY timestamp_ms DESC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![channel_id, limit as i64], |row| {
            let att_json: String = row.get(4)?;
            let attachments: Vec<String> = serde_json::from_str(&att_json).unwrap_or_default();
            Ok(StoredMessage {
                id: row.get(0)?,
                channel_id: row.get(1)?,
                author_id: row.get(2)?,
                content: row.get(3)?,
                attachments,
                timestamp_ms: row.get(5)?,
                signature_hex: row.get(6)?,
            })
        })?;

        let mut messages = Vec::new();
        for r in rows {
            messages.push(r?);
        }
        messages.reverse(); // Ascending chronological order
        Ok(messages)
    }

    // --- User Audio Preferences (Volume per participant) ---
    pub fn set_user_volume(&self, user_id: &str, volume: f32) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO user_audio_settings (user_id, volume, is_muted)
            VALUES (?1, ?2, 0)
            ON CONFLICT(user_id) DO UPDATE SET volume = ?2
            "#,
            params![user_id, volume],
        )?;
        Ok(())
    }

    pub fn get_user_volume(&self, user_id: &str) -> Result<f32, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT volume FROM user_audio_settings WHERE user_id = ?1")?;
        let vol = stmt
            .query_row(params![user_id], |row| row.get::<_, f32>(0))
            .optional()?;
        Ok(vol.unwrap_or(1.0))
    }

    // --- CRDT Snapshots Persistence ---
    pub fn save_crdt_snapshot(&self, doc_id: &str, snapshot: &[u8]) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        conn.execute(
            r#"
            INSERT INTO crdt_snapshots (doc_id, snapshot_bytes, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(doc_id) DO UPDATE SET
                snapshot_bytes = excluded.snapshot_bytes,
                updated_at = excluded.updated_at
            "#,
            params![doc_id, snapshot, now],
        )?;
        Ok(())
    }

    pub fn load_crdt_snapshot(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT snapshot_bytes FROM crdt_snapshots WHERE doc_id = ?1")?;
        let bytes = stmt
            .query_row(params![doc_id], |row| row.get::<_, Vec<u8>>(0))
            .optional()?;
        Ok(bytes)
    }
}

/// Loro CRDT document wrapper for local-first peer-to-peer data synchronization.
/// Manages channel hierarchies and deterministic message resolution across disconnected peers.
pub struct CrdtChannelDoc {
    doc: LoroDoc,
}

impl CrdtChannelDoc {
    pub fn new() -> Self {
        Self { doc: LoroDoc::new() }
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, StorageError> {
        let doc = LoroDoc::new();
        doc.import(bytes)?;
        Ok(Self { doc })
    }

    /// Append a new message into the channel CRDT list.
    pub fn append_message(&self, msg: &StoredMessage) -> Result<(), StorageError> {
        let list: LoroList = self.doc.get_list("messages");
        let json_val = serde_json::to_value(msg)?;
        let loro_val = LoroValue::from(json_val);
        list.push(loro_val)?;
        self.doc.commit();
        Ok(())
    }

    /// Export delta updates since a known version vector for fast P2P gossip sync.
    pub fn export_updates_from(&self, vv: &VersionVector) -> Result<Vec<u8>, StorageError> {
        let bytes = self.doc.export(loro::ExportMode::updates(vv))?;
        Ok(bytes)
    }

    /// Export full document snapshot for initial peer onboarding.
    pub fn export_snapshot(&self) -> Result<Vec<u8>, StorageError> {
        let bytes = self.doc.export(loro::ExportMode::Snapshot)?;
        Ok(bytes)
    }

    /// Import updates or snapshot received from another peer over QUIC/libp2p.
    pub fn import_data(&self, data: &[u8]) -> Result<(), StorageError> {
        self.doc.import(data)?;
        Ok(())
    }

    /// Current document version vector.
    pub fn version(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    /// Read all messages resolved by the CRDT.
    pub fn get_messages(&self) -> Vec<StoredMessage> {
        let list = self.doc.get_list("messages");
        let mut msgs = Vec::new();

        if let LoroValue::List(items) = list.get_value() {
            for val in items.iter() {
                if let Ok(msg) = serde_json::from_value::<StoredMessage>(val.clone().to_json_value()) {
                    msgs.push(msg);
                }
            }
        }
        msgs
    }
}

impl Default for CrdtChannelDoc {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_db_contacts_and_channels() {
        let db = LocalDatabase::in_memory().unwrap();
        let card = ContactCard {
            user_id: "bala:1234567890abcdef".to_string(),
            public_key_hex: "1234567890abcdef".to_string(),
            display_name: "GamerPro".to_string(),
            avatar_url: None,
            bio: Some("P2P enthusiast".to_string()),
        };

        db.save_contact(&card).unwrap();
        let fetched = db.get_contact("bala:1234567890abcdef").unwrap().unwrap();
        assert_eq!(fetched.display_name, "GamerPro");

        let ch = StoredChannel {
            id: "chan-general".to_string(),
            server_id: Some("srv-1".to_string()),
            name: "general-chat".to_string(),
            channel_type: "text".to_string(),
            topic: Some("Discussion".to_string()),
            created_at: 1000,
        };
        db.save_channel(&ch).unwrap();

        let channels = db.list_channels(Some("srv-1")).unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "general-chat");

        db.delete_channel("chan-general").unwrap();
        assert_eq!(db.list_channels(Some("srv-1")).unwrap().len(), 0);
    }

    #[test]
    fn test_crdt_sync_between_two_peers() {
        let peer_a = CrdtChannelDoc::new();
        let peer_b = CrdtChannelDoc::new();

        let msg1 = StoredMessage {
            id: "m-1".to_string(),
            channel_id: "c-1".to_string(),
            author_id: "bala:peerA".to_string(),
            content: "Hello from Peer A".to_string(),
            attachments: vec![],
            timestamp_ms: 100,
            signature_hex: None,
        };
        peer_a.append_message(&msg1).unwrap();

        // Export delta from A and import to B
        let vv_b = peer_b.version();
        let delta = peer_a.export_updates_from(&vv_b).unwrap();
        peer_b.import_data(&delta).unwrap();

        let b_msgs = peer_b.get_messages();
        assert_eq!(b_msgs.len(), 1);
        assert_eq!(b_msgs[0].content, "Hello from Peer A");
    }
}
