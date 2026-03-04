// Types matching Rust piper-core types that cross the Tauri bridge.
// SessionEvent is serialized as externally-tagged serde JSON:
//   { "Ready": { "our_id": "...", "ticket_str": "..." } }

// ── Primitives ──────────────────────────────────────────────────────────────

/** Hex-encoded BLAKE3 hash (64 chars). iroh_blobs::Hash serializes as hex. */
export type HashHex = string;

/** Base32-encoded endpoint ID. iroh::EndpointId serializes as base32. */
export type EndpointIdStr = string;

/** 16-byte message ID as array of numbers. */
export type MessageId = number[];

// ── Connection type ─────────────────────────────────────────────────────────

export type ConnType = "Unknown" | "Direct" | "Relay" | "You";

// ── File offer ──────────────────────────────────────────────────────────────

export interface FileOffer {
  sender_nickname: string;
  sender_id: EndpointIdStr;
  filename: string;
  size: number;
  hash: HashHex;
}

// ── Transfer state ──────────────────────────────────────────────────────────

export type TransferState =
  | "Pending"
  | { Downloading: { bytes_received: number; total_bytes: number } }
  | { Complete: string } // path
  | { Failed: string } // error
  | "Sharing";

export interface TransferEntry {
  offer: FileOffer;
  state: TransferState;
}

// ── History ─────────────────────────────────────────────────────────────────

export type HistoryEntryKind =
  | { Chat: { nickname: string; text: string } }
  | {
      FileOffer: {
        nickname: string;
        endpoint_id: EndpointIdStr;
        filename: string;
        size: number;
        hash: number[]; // [u8; 32]
        mime_type: string | null;
        target: string | null;
      };
    }
  | { FileRetract: { hash: number[] } }
  | { System: string };

export interface HistoryEntry {
  message_id: MessageId;
  timestamp_ms: number;
  kind: HistoryEntryKind;
}

// ── Session event payloads ──────────────────────────────────────────────────
// Each event name maps to a specific variant of SessionEvent.
// Tauri emits the full enum variant as the payload.

export interface ReadyPayload {
  Ready: {
    our_id: EndpointIdStr;
    ticket_str: string;
  };
}

export interface PeerJoinedPayload {
  PeerJoined: {
    endpoint_id: EndpointIdStr;
    nickname: string;
  };
}

export interface PeerLeftPayload {
  PeerLeft: {
    endpoint_id: EndpointIdStr;
    nickname: string;
  };
}

export interface ChatReceivedPayload {
  ChatReceived: {
    nickname: string;
    text: string;
    message_id: MessageId;
    timestamp_ms: number;
  };
}

export interface ChatSentPayload {
  ChatSent: {
    nickname: string;
    text: string;
    message_id: MessageId;
    timestamp_ms: number;
  };
}

export interface FileOfferedPayload {
  FileOffered: {
    offer: FileOffer;
    message_id: MessageId;
    timestamp_ms: number;
    target: string | null;
  };
}

export interface FileRetractedPayload {
  FileRetracted: {
    nickname: string;
    hash: HashHex;
    filename: string | null;
  };
}

export interface FileSharedPayload {
  FileShared: {
    offer: FileOffer;
    target: string | null;
  };
}

export interface FileShareFailedPayload {
  FileShareFailed: {
    error: string;
  };
}

export interface TransferProgressPayload {
  TransferProgress: {
    hash: HashHex;
    bytes_received: number;
    total_bytes: number;
  };
}

export interface TransferCompletePayload {
  TransferComplete: {
    hash: HashHex;
    filename: string;
    path: string;
  };
}

export interface TransferFailedPayload {
  TransferFailed: {
    hash: HashHex;
    filename: string;
    error: string;
  };
}

export interface ConnTypeChangedPayload {
  ConnTypeChanged: {
    endpoint_id: EndpointIdStr;
    conn_type: ConnType;
  };
}

export interface HistorySyncedPayload {
  HistorySynced: {
    entries: HistoryEntry[];
    merged_count: number;
  };
}

export interface SystemPayload {
  System: {
    message: string;
  };
}

export interface DisconnectedPayload {
  Disconnected: {
    reason: string;
  };
}

// ── App-level types ─────────────────────────────────────────────────────────

export interface Peer {
  endpointId: EndpointIdStr;
  nickname: string;
  connType: ConnType;
}

export interface ChatMessage {
  id: string; // unique display key
  kind: "system" | "chat" | "file" | "ticket";
  nickname?: string;
  text: string;
  timestamp?: number;
}
