//! Networking primitives: wire protocol, tickets, and connection tracking.
//!
//! This module contains all the types that cross the network boundary —
//! the messages peers send each other, the ticket that bootstraps a room,
//! and the hook that tracks whether connections are direct or relayed.

// Standard library imports — grouped by module as is idiomatic in Rust.
// `HashMap` is a hash-based map; `BTreeSet` is a sorted set backed by a B-tree.
use std::collections::{BTreeSet, HashMap};
// `Arc` (Atomic Reference Counted) enables shared ownership across threads.
// `RwLock` allows many concurrent readers OR one exclusive writer.
use std::sync::{Arc, RwLock};

// `anyhow::Result` is a convenient alias for `Result<T, anyhow::Error>`.
// It lets any error type that implements `std::error::Error` be returned with `?`.
use anyhow::Result;
// Iroh endpoint types — `EndpointHooks` is a *trait* (Rust's interface/protocol)
// that lets us intercept connection lifecycle events.
use iroh::endpoint::{AfterHandshakeOutcome, ConnectionInfo, EndpointHooks};
// `EndpointId` is a unique cryptographic identifier for each peer node.
use iroh::EndpointId;
// `TopicId` identifies a gossip topic (chat room) — a 32-byte hash.
use iroh_gossip::proto::TopicId;
// The `Ticket` trait from iroh provides base32 serialization for sharing
// connection info out-of-band (e.g. pasting a string into another terminal).
use iroh_tickets::Ticket;
// `Serialize` and `Deserialize` are derive macros from the `serde` crate.
// They auto-generate code to convert structs/enums to/from formats like JSON,
// postcard (binary), etc. — a cornerstone of Rust's zero-boilerplate approach.
use serde::{Deserialize, Serialize};

// ── Message identity & timestamps ────────────────────────────────────────────

/// A 128-bit random message identifier for deduplication during history merge.
pub type MessageId = [u8; 16];

/// Generate a new random 128-bit message ID.
pub fn new_message_id() -> MessageId {
    rand::random()
}

/// Current wall-clock time as milliseconds since UNIX epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// ── Wire protocol ────────────────────────────────────────────────────────────
//
// Every message sent over the gossip network is one of these variants.
// We use `postcard` (a compact binary format) to serialize them.

/// Messages exchanged between peers over the gossip network.
///
/// This is a Rust *enum* with named fields — sometimes called a "tagged union"
/// or "algebraic data type". Each variant is a distinct message kind, and
/// pattern matching (`match`) ensures you handle every case.
///
/// The `#[derive(...)]` attribute invokes procedural macros at compile time
/// to auto-implement the `Serialize` and `Deserialize` traits. No runtime
/// reflection — all the serialization code is generated at compile time.
#[derive(Serialize, Deserialize)]
pub enum Message {
    /// Sent when a peer first connects, so others learn its display name.
    Join {
        nickname: String,
        endpoint_id: EndpointId,
    },
    /// A regular chat message from a peer.
    Chat {
        nickname: String,
        text: String,
        message_id: MessageId,
        timestamp_ms: u64,
    },
    /// A file offer — the sender has imported a file into their blob store
    /// and is advertising it so peers can download via iroh-blobs.
    FileOffer {
        nickname: String,
        endpoint_id: EndpointId,
        filename: String,
        size: u64,
        /// The BLAKE3 hash of the file content, stored as raw bytes for
        /// compact serialization with postcard.
        hash: [u8; 32],
        message_id: MessageId,
        timestamp_ms: u64,
        /// MIME type inferred from the file extension (e.g. "image/png").
        mime_type: Option<String>,
        /// If `Some(name)`, only the named peer should accept this offer.
        /// `None` means broadcast to all peers.
        target: Option<String>,
    },
    /// Retract a previously shared file offer.
    FileRetract {
        nickname: String,
        hash: [u8; 32],
        message_id: MessageId,
        timestamp_ms: u64,
    },
    /// A peer is offering its chat history as a downloadable blob.
    HistoryOffer {
        message_count: u32,
        oldest_timestamp_ms: u64,
        newest_timestamp_ms: u64,
        hash: [u8; 32],
        endpoint_id: EndpointId,
    },
}

// ── History entry (serializable storage format) ──────────────────────────────

/// A single entry in the chat history log, serialized into a blob for
/// history sync. Separate from the wire `Message` enum so we can evolve
/// the storage format independently.
#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryEntry {
    pub message_id: MessageId,
    pub timestamp_ms: u64,
    pub kind: HistoryEntryKind,
}

/// The payload of a history entry.
#[derive(Serialize, Deserialize, Clone)]
pub enum HistoryEntryKind {
    Chat {
        nickname: String,
        text: String,
    },
    FileOffer {
        nickname: String,
        endpoint_id: EndpointId,
        filename: String,
        size: u64,
        hash: [u8; 32],
        mime_type: Option<String>,
        target: Option<String>,
    },
    FileRetract {
        hash: [u8; 32],
    },
    System(String),
}

// ── Ticket ───────────────────────────────────────────────────────────────────
//
// A `ChatTicket` is shared out-of-band (copy-paste) to let others join a room.
// It encodes the gossip topic ID plus a set of known peers to bootstrap from.

/// Ticket containing everything needed to join a chat room.
///
/// `#[derive(Clone)]` generates a `.clone()` method that deep-copies the struct.
/// This is needed because we modify a copy of the ticket (to add our own
/// endpoint) without mutating the original.
///
/// Struct fields are `pub` because `main.rs` needs to read/write `bootstrap`
/// and `topic_id` directly. In Rust, visibility is *module-scoped* by default —
/// everything is private unless marked `pub`.
#[derive(Serialize, Deserialize, Clone)]
pub struct ChatTicket {
    pub topic_id: TopicId,
    /// `BTreeSet` keeps endpoint IDs sorted and deduplicated. Unlike `HashSet`,
    /// iteration order is deterministic, which gives consistent serialization.
    pub bootstrap: BTreeSet<EndpointId>,
}

impl ChatTicket {
    /// Create a ticket for a brand-new chat room with a random topic ID.
    ///
    /// `Self` is a type alias for the impl's type (`ChatTicket`). Using `Self`
    /// means if you rename the struct, this code still compiles.
    ///
    /// `rand::random()` returns a `[u8; 32]` here — Rust infers the array size
    /// from `TopicId::from_bytes`'s parameter type. Type inference in Rust
    /// flows both forward (from arguments) and backward (from expected return).
    pub fn new_random() -> Self {
        Self {
            topic_id: TopicId::from_bytes(rand::random()),
            bootstrap: BTreeSet::new(),
        }
    }
}

/// Implement the iroh `Ticket` trait so `ChatTicket` can be serialized to a
/// human-friendly base32 string (for copy-paste in the terminal).
///
/// Trait implementations in Rust are separate `impl` blocks from the type's
/// inherent methods — this is how Rust achieves polymorphism without
/// inheritance. Any type can implement any trait (subject to orphan rules).
///
/// `const KIND` is an *associated constant* — a value tied to the trait
/// implementation rather than to any particular instance.
impl Ticket for ChatTicket {
    const KIND: &'static str = "chat";

    /// Serialize to bytes using postcard (a compact, no-std-friendly binary format).
    /// `.unwrap()` panics on failure — safe here because serialization of
    /// known-good types never fails with postcard.
    fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).unwrap()
    }

    /// Deserialize from bytes. Returns a `ParseError` on invalid input.
    /// The `?` operator converts postcard's error into `ParseError` automatically
    /// because `ParseError` implements `From<postcard::Error>`.
    fn from_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        Ok(postcard::from_bytes(bytes)?)
    }
}

// ── Connection tracking ─────────────────────────────────────────────────
//
// Iroh connections can be "direct" (UDP hole-punched) or "relayed" through a
// DERP server. We track which type each peer uses, updating on every handshake.

/// Whether a peer connection is direct (IP), relayed, or not yet determined.
///
/// Iroh's QUIC connections start as relayed (through a DERP relay server) and
/// may upgrade to direct (UDP hole-punched) once both peers discover each other's
/// public IP. This enum tracks the current state for display in the peers panel.
pub enum ConnType {
    /// Connection type not yet determined (peer just connected).
    Unknown,
    /// Direct UDP connection — lowest latency, no relay overhead.
    Direct,
    /// Traffic is being relayed through a DERP server — higher latency but
    /// works even when both peers are behind restrictive NATs.
    Relay,
}

/// Display information about a connected peer.
///
/// This struct bundles the peer's display name with their connection type.
/// It's stored in `App.peers` (a `BTreeMap<EndpointId, PeerInfo>`) and
/// rendered in the peers sidebar.
pub struct PeerInfo {
    /// Display name — either their chosen nickname (after receiving a Join message)
    /// or a short hex prefix of their endpoint ID (before they identify themselves).
    pub name: String,
    /// Current connection type — updated periodically from the `ConnTracker`.
    pub conn_type: ConnType,
}

/// Thread-safe connection tracker using interior mutability.
///
/// This is a *newtype pattern* — a single-field tuple struct that wraps an
/// inner type to give it a distinct name and impl blocks. The inner type
/// `Arc<RwLock<HashMap<...>>>` combines three Rust concurrency primitives:
///
/// - `Arc` (Atomic Reference Count): shared ownership across threads. Cloning
///   an Arc increments a counter; dropping it decrements. When the count hits
///   zero the inner value is dropped.
/// - `RwLock`: allows many concurrent readers (`read()`) or one exclusive
///   writer (`write()`). Unlike `Mutex`, readers don't block each other.
/// - `HashMap`: the actual key→value store mapping endpoint IDs to connection
///   info.
///
/// `#[derive(Debug)]` auto-generates a `Debug` implementation so the struct
/// can be printed with `{:?}` formatting — useful for logging.
#[derive(Debug)]
pub struct ConnTracker(Arc<RwLock<HashMap<EndpointId, ConnectionInfo>>>);

impl ConnTracker {
    /// Create a new empty tracker.
    ///
    /// `Arc::default()` creates an `Arc<RwLock<HashMap<...>>>` where the
    /// HashMap is empty. Rust infers all the generic types from the struct's
    /// field type.
    pub fn new() -> Self {
        Self(Arc::default())
    }

    /// Create a hook that shares the same backing map.
    ///
    /// `self.0` accesses the first (only) field of a tuple struct.
    /// `.clone()` on an `Arc` is cheap — it just increments the reference
    /// count. Both the `ConnTracker` and the returned `ConnTrackerHook` will
    /// point to the same underlying `RwLock<HashMap<...>>`.
    pub fn hook(&self) -> ConnTrackerHook {
        ConnTrackerHook(self.0.clone())
    }

    /// Look up the connection type for a given peer.
    ///
    /// This demonstrates Rust's *match guard* syntax: `Some(p) if p.is_ip()`
    /// matches only when the guard condition is true. Guards provide a way to
    /// add arbitrary boolean conditions to match arms.
    ///
    /// `.and_then()` is a combinator on `Option` — it chains a closure that
    /// itself returns an `Option`, flattening `Option<Option<T>>` to `Option<T>`.
    pub fn conn_type(&self, id: &EndpointId) -> ConnType {
        let map = self.0.read().unwrap();
        match map.get(id).and_then(|c| c.selected_path()) {
            Some(p) if p.is_ip() => ConnType::Direct,
            Some(_) => ConnType::Relay,
            None => ConnType::Unknown,
        }
    }
}

/// Endpoint hook that records connection info after each QUIC handshake.
///
/// This is a separate newtype (rather than making `ConnTracker` implement
/// `EndpointHooks` directly) because the hook needs to be `Send + Sync` and
/// move into the iroh endpoint, while `ConnTracker` stays with the main thread.
#[derive(Debug)]
pub struct ConnTrackerHook(Arc<RwLock<HashMap<EndpointId, ConnectionInfo>>>);

/// Implement the `EndpointHooks` trait to intercept new connections.
///
/// The lifetime annotations `'a` here tell the compiler that:
/// - The returned `Future` borrows `self` and `conn` for lifetime `'a`
/// - The future must not outlive either of those borrows
///
/// `impl Future<...> + Send + 'a` is Rust's "return-position impl Trait"
/// syntax — it means "I return *some* type that implements Future, is Send,
/// and lives at least as long as 'a". The caller doesn't know the concrete
/// type (it's opaque), which lets the compiler optimize away the vtable.
impl EndpointHooks for ConnTrackerHook {
    fn after_handshake<'a>(
        &'a self,
        conn: &'a ConnectionInfo,
    ) -> impl std::future::Future<Output = AfterHandshakeOutcome> + Send + 'a {
        // `.write().unwrap()` acquires the write lock (panics if poisoned).
        // We insert the connection info keyed by the remote peer's ID.
        self.0
            .write()
            .unwrap()
            .insert(conn.remote_id(), conn.clone());
        // Return a future that immediately resolves to "accept the connection".
        // `async { value }` creates a zero-cost future that yields `value`.
        async { AfterHandshakeOutcome::accept() }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// `#[cfg(test)]` means this module is only compiled when running `cargo test`.
// It won't bloat the release binary. Test modules conventionally live at the
// bottom of the file they test and have access to all private items in the
// parent module.

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that a `ChatTicket` survives a serialize→deserialize round-trip.
    ///
    /// This verifies our `Ticket` trait implementation (to_bytes/from_bytes)
    /// produces consistent results — critical since tickets are copy-pasted
    /// between users.
    #[test]
    fn ticket_roundtrip() {
        let original = ChatTicket::new_random();
        let bytes = original.to_bytes();
        let decoded = ChatTicket::from_bytes(&bytes).expect("should decode");
        assert_eq!(original.topic_id, decoded.topic_id);
        assert_eq!(original.bootstrap, decoded.bootstrap);
    }

    /// Test the human-friendly base32 serialization provided by the `Ticket`
    /// trait's `serialize`/`deserialize` methods (which wrap to_bytes/from_bytes).
    #[test]
    fn ticket_base32_roundtrip() {
        let original = ChatTicket::new_random();
        let base32_str = <ChatTicket as Ticket>::serialize(&original);
        let decoded =
            <ChatTicket as Ticket>::deserialize(&base32_str).expect("should decode base32");
        assert_eq!(original.topic_id, decoded.topic_id);
    }

    /// Verify that invalid base32 strings produce an error rather than panicking.
    #[test]
    fn ticket_deserialize_invalid() {
        let result = <ChatTicket as Ticket>::deserialize("not-a-valid-ticket");
        assert!(result.is_err());
    }

    /// Test that `Message::Chat` survives a postcard round-trip.
    #[test]
    fn message_chat_roundtrip() {
        let mid = new_message_id();
        let msg = Message::Chat {
            nickname: "Alice".into(),
            text: "hello!".into(),
            message_id: mid,
            timestamp_ms: 1700000000000,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::Chat {
                nickname,
                text,
                message_id,
                timestamp_ms,
            } => {
                assert_eq!(nickname, "Alice");
                assert_eq!(text, "hello!");
                assert_eq!(message_id, mid);
                assert_eq!(timestamp_ms, 1700000000000);
            }
            _ => panic!("expected Chat variant"),
        }
    }

    /// Test that `Message::Join` survives a postcard round-trip.
    #[test]
    fn message_join_roundtrip() {
        let id = iroh::EndpointId::from_bytes(&[1u8; 32]).unwrap();
        let msg = Message::Join {
            nickname: "Bob".into(),
            endpoint_id: id,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::Join {
                nickname,
                endpoint_id,
            } => {
                assert_eq!(nickname, "Bob");
                assert_eq!(endpoint_id, id);
            }
            _ => panic!("expected Join variant"),
        }
    }

    /// Test that `Message::FileOffer` survives a postcard round-trip.
    #[test]
    fn message_file_offer_roundtrip() {
        let id = iroh::EndpointId::from_bytes(&[3u8; 32]).unwrap();
        let hash = [7u8; 32];
        let mid = new_message_id();
        let msg = Message::FileOffer {
            nickname: "Alice".into(),
            endpoint_id: id,
            filename: "photo.png".into(),
            size: 123456,
            hash,
            message_id: mid,
            timestamp_ms: 1700000000000,
            mime_type: Some("image/png".into()),
            target: None,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::FileOffer {
                nickname,
                endpoint_id,
                filename,
                size,
                hash: h,
                message_id,
                timestamp_ms,
                mime_type,
                target,
            } => {
                assert_eq!(nickname, "Alice");
                assert_eq!(endpoint_id, id);
                assert_eq!(filename, "photo.png");
                assert_eq!(size, 123456);
                assert_eq!(h, hash);
                assert_eq!(message_id, mid);
                assert_eq!(timestamp_ms, 1700000000000);
                assert_eq!(mime_type, Some("image/png".into()));
                assert_eq!(target, None);
            }
            _ => panic!("expected FileOffer variant"),
        }
    }

    /// Test that `Message::HistoryOffer` survives a postcard round-trip.
    #[test]
    fn message_history_offer_roundtrip() {
        let id = iroh::EndpointId::from_bytes(&[1u8; 32]).unwrap();
        let hash = [9u8; 32];
        let msg = Message::HistoryOffer {
            message_count: 42,
            oldest_timestamp_ms: 1700000000000,
            newest_timestamp_ms: 1700000060000,
            hash,
            endpoint_id: id,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let decoded: Message = postcard::from_bytes(&bytes).unwrap();
        match decoded {
            Message::HistoryOffer {
                message_count,
                oldest_timestamp_ms,
                newest_timestamp_ms,
                hash: h,
                endpoint_id,
            } => {
                assert_eq!(message_count, 42);
                assert_eq!(oldest_timestamp_ms, 1700000000000);
                assert_eq!(newest_timestamp_ms, 1700000060000);
                assert_eq!(h, hash);
                assert_eq!(endpoint_id, id);
            }
            _ => panic!("expected HistoryOffer variant"),
        }
    }

    /// Test `HistoryEntry` postcard round-trip.
    #[test]
    fn history_entry_roundtrip() {
        let mid = new_message_id();
        let entry = HistoryEntry {
            message_id: mid,
            timestamp_ms: 1700000000000,
            kind: HistoryEntryKind::Chat {
                nickname: "Bob".into(),
                text: "hi".into(),
            },
        };
        let bytes = postcard::to_stdvec(&entry).unwrap();
        let decoded: HistoryEntry = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.message_id, mid);
        assert_eq!(decoded.timestamp_ms, 1700000000000);
        match decoded.kind {
            HistoryEntryKind::Chat { nickname, text } => {
                assert_eq!(nickname, "Bob");
                assert_eq!(text, "hi");
            }
            _ => panic!("expected Chat kind"),
        }
    }

    /// Test `Vec<HistoryEntry>` round-trip (the history blob format).
    #[test]
    fn history_vec_roundtrip() {
        let entries = vec![
            HistoryEntry {
                message_id: new_message_id(),
                timestamp_ms: 1000,
                kind: HistoryEntryKind::System("room created".into()),
            },
            HistoryEntry {
                message_id: new_message_id(),
                timestamp_ms: 2000,
                kind: HistoryEntryKind::Chat {
                    nickname: "Alice".into(),
                    text: "hello".into(),
                },
            },
        ];
        let bytes = postcard::to_stdvec(&entries).unwrap();
        let decoded: Vec<HistoryEntry> = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].timestamp_ms, 1000);
        assert_eq!(decoded[1].timestamp_ms, 2000);
    }

    /// Test that `ConnTracker::new()` starts empty and returns `Unknown` for
    /// any peer.
    #[test]
    fn conn_tracker_unknown_by_default() {
        let tracker = ConnTracker::new();
        let id = iroh::EndpointId::from_bytes(&[42u8; 32]).unwrap();
        // A freshly-created tracker has no entries, so all lookups return Unknown
        assert!(matches!(tracker.conn_type(&id), ConnType::Unknown));
    }
}
