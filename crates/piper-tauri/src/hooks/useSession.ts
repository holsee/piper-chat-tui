import { useReducer, useEffect, useCallback, useRef } from "react";
import { listenAll } from "../api/events";
import * as commands from "../api/commands";
import type {
  ChatMessage,
  Peer,
  TransferEntry,
  FileOffer,
  ConnType,
  HistoryEntry,
  ReadyPayload,
  PeerJoinedPayload,
  PeerLeftPayload,
  ChatReceivedPayload,
  ChatSentPayload,
  FileOfferedPayload,
  FileRetractedPayload,
  FileSharedPayload,
  FileShareFailedPayload,
  TransferProgressPayload,
  TransferCompletePayload,
  TransferFailedPayload,
  ConnTypeChangedPayload,
  HistorySyncedPayload,
  SystemPayload,
  DisconnectedPayload,
} from "../types";

// ── State ───────────────────────────────────────────────────────────────────

export interface SessionState {
  screen: "welcome" | "chat";
  messages: ChatMessage[];
  peers: Map<string, Peer>;
  transfers: TransferEntry[];
  ticketStr: string | null;
  ourId: string | null;
  nickname: string;
  connected: boolean;
  error: string | null;
}

const initialState: SessionState = {
  screen: "welcome",
  messages: [],
  peers: new Map(),
  transfers: [],
  ticketStr: null,
  ourId: null,
  nickname: "",
  connected: false,
  error: null,
};

// ── Actions ─────────────────────────────────────────────────────────────────

type Action =
  | { type: "SESSION_STARTED"; nickname: string }
  | { type: "READY"; payload: ReadyPayload }
  | { type: "PEER_JOINED"; payload: PeerJoinedPayload }
  | { type: "PEER_LEFT"; payload: PeerLeftPayload }
  | { type: "CHAT_RECEIVED"; payload: ChatReceivedPayload }
  | { type: "CHAT_SENT"; payload: ChatSentPayload }
  | { type: "FILE_OFFERED"; payload: FileOfferedPayload }
  | { type: "FILE_RETRACTED"; payload: FileRetractedPayload }
  | { type: "FILE_SHARED"; payload: FileSharedPayload }
  | { type: "FILE_SHARE_FAILED"; payload: FileShareFailedPayload }
  | { type: "TRANSFER_PROGRESS"; payload: TransferProgressPayload }
  | { type: "TRANSFER_COMPLETE"; payload: TransferCompletePayload }
  | { type: "TRANSFER_FAILED"; payload: TransferFailedPayload }
  | { type: "CONN_TYPE_CHANGED"; payload: ConnTypeChangedPayload }
  | { type: "HISTORY_SYNCED"; payload: HistorySyncedPayload }
  | { type: "SYSTEM"; payload: SystemPayload }
  | { type: "DISCONNECTED"; payload: DisconnectedPayload }
  | { type: "CLEAR_ERROR" };

// ── Helpers ─────────────────────────────────────────────────────────────────

let msgCounter = 0;
function nextId(): string {
  return `msg-${++msgCounter}`;
}

export function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function addFileOffer(
  transfers: TransferEntry[],
  offer: FileOffer,
  state: "Pending" | "Sharing"
): TransferEntry[] {
  return [...transfers, { offer, state }];
}

function historyToMessages(entries: HistoryEntry[]): ChatMessage[] {
  const msgs: ChatMessage[] = [];
  for (const entry of entries) {
    const kind = entry.kind;
    if ("Chat" in kind) {
      msgs.push({
        id: nextId(),
        kind: "chat",
        nickname: kind.Chat.nickname,
        text: kind.Chat.text,
        timestamp: entry.timestamp_ms,
      });
    } else if ("FileOffer" in kind) {
      const fo = kind.FileOffer;
      const targetInfo = fo.target ? ` (to ${fo.target})` : "";
      msgs.push({
        id: nextId(),
        kind: "file",
        nickname: fo.nickname,
        text: `shared ${fo.filename} (${formatFileSize(fo.size)})${targetInfo}`,
        timestamp: entry.timestamp_ms,
      });
    } else if ("FileRetract" in kind) {
      msgs.push({
        id: nextId(),
        kind: "system",
        text: "a file was unshared",
        timestamp: entry.timestamp_ms,
      });
    } else if ("System" in kind) {
      msgs.push({
        id: nextId(),
        kind: "system",
        text: kind.System,
        timestamp: entry.timestamp_ms,
      });
    }
  }
  return msgs;
}

// ── Reducer ─────────────────────────────────────────────────────────────────

function reducer(state: SessionState, action: Action): SessionState {
  switch (action.type) {
    case "SESSION_STARTED":
      return {
        ...state,
        screen: "chat",
        nickname: action.nickname,
        messages: [],
        peers: new Map(),
        transfers: [],
        connected: true,
        error: null,
      };

    case "READY": {
      const { our_id, ticket_str } = action.payload.Ready;
      const peers = new Map(state.peers);
      peers.set(our_id, {
        endpointId: our_id,
        nickname: `${state.nickname} (you)`,
        connType: "You" as ConnType,
      });
      return {
        ...state,
        ourId: our_id,
        ticketStr: ticket_str,
        peers,
        messages: [
          ...state.messages,
          { id: nextId(), kind: "system", text: "Session ready" },
          {
            id: nextId(),
            kind: "ticket",
            text: ticket_str,
          },
        ],
      };
    }

    case "PEER_JOINED": {
      const { endpoint_id, nickname } = action.payload.PeerJoined;
      const peers = new Map(state.peers);
      peers.set(endpoint_id, {
        endpointId: endpoint_id,
        nickname,
        connType: "Unknown" as ConnType,
      });
      return {
        ...state,
        peers,
        messages: [
          ...state.messages,
          { id: nextId(), kind: "system", text: `${nickname} joined` },
        ],
      };
    }

    case "PEER_LEFT": {
      const { endpoint_id, nickname } = action.payload.PeerLeft;
      const peers = new Map(state.peers);
      peers.delete(endpoint_id);
      return {
        ...state,
        peers,
        messages: [
          ...state.messages,
          { id: nextId(), kind: "system", text: `${nickname} left` },
        ],
      };
    }

    case "CHAT_RECEIVED": {
      const { nickname, text, timestamp_ms } =
        action.payload.ChatReceived;
      return {
        ...state,
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "chat",
            nickname,
            text,
            timestamp: timestamp_ms,
          },
        ],
      };
    }

    case "CHAT_SENT": {
      const { nickname, text, timestamp_ms } = action.payload.ChatSent;
      return {
        ...state,
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "chat",
            nickname,
            text,
            timestamp: timestamp_ms,
          },
        ],
      };
    }

    case "FILE_OFFERED": {
      const { offer, timestamp_ms, target } =
        action.payload.FileOffered;
      const targetInfo = target ? ` (to ${target})` : "";
      return {
        ...state,
        transfers: addFileOffer(state.transfers, offer, "Pending"),
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "file",
            nickname: offer.sender_nickname,
            text: `shared ${offer.filename} (${formatFileSize(offer.size)})${targetInfo}`,
            timestamp: timestamp_ms,
          },
        ],
      };
    }

    case "FILE_RETRACTED": {
      const { nickname, hash, filename } =
        action.payload.FileRetracted;
      return {
        ...state,
        transfers: state.transfers.filter((t) => t.offer.hash !== hash),
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "system",
            text: `${nickname} unshared ${filename ?? "a file"}`,
          },
        ],
      };
    }

    case "FILE_SHARED": {
      const { offer, target } = action.payload.FileShared;
      const targetInfo = target ? ` to ${target}` : "";
      return {
        ...state,
        transfers: addFileOffer(state.transfers, offer, "Sharing"),
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "system",
            text: `You shared ${offer.filename}${targetInfo}`,
          },
        ],
      };
    }

    case "FILE_SHARE_FAILED": {
      const { error } = action.payload.FileShareFailed;
      return {
        ...state,
        error: `File share failed: ${error}`,
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "system",
            text: `File share failed: ${error}`,
          },
        ],
      };
    }

    case "TRANSFER_PROGRESS": {
      const { hash, bytes_received, total_bytes } =
        action.payload.TransferProgress;
      return {
        ...state,
        transfers: state.transfers.map((t) =>
          t.offer.hash === hash
            ? {
                ...t,
                state: { Downloading: { bytes_received, total_bytes } },
              }
            : t
        ),
      };
    }

    case "TRANSFER_COMPLETE": {
      const { hash, filename, path } = action.payload.TransferComplete;
      return {
        ...state,
        transfers: state.transfers.map((t) =>
          t.offer.hash === hash ? { ...t, state: { Complete: path } } : t
        ),
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "system",
            text: `Downloaded ${filename}`,
          },
        ],
      };
    }

    case "TRANSFER_FAILED": {
      const { hash, filename, error } = action.payload.TransferFailed;
      return {
        ...state,
        transfers: state.transfers.map((t) =>
          t.offer.hash === hash
            ? { ...t, state: { Failed: error } }
            : t
        ),
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "system",
            text: `Download failed: ${filename} — ${error}`,
          },
        ],
      };
    }

    case "CONN_TYPE_CHANGED": {
      const { endpoint_id, conn_type } =
        action.payload.ConnTypeChanged;
      const peers = new Map(state.peers);
      const peer = peers.get(endpoint_id);
      if (peer) {
        peers.set(endpoint_id, { ...peer, connType: conn_type });
      }
      return { ...state, peers };
    }

    case "HISTORY_SYNCED": {
      const { entries, merged_count } = action.payload.HistorySynced;
      if (merged_count === 0) return state;
      const historyMsgs = historyToMessages(entries);
      // Also add file offers from history to transfers
      const newTransfers = [...state.transfers];
      for (const entry of entries) {
        if ("FileOffer" in entry.kind) {
          const fo = entry.kind.FileOffer;
          // Convert [u8; 32] array to hex string
          const hashHex = fo.hash
            .map((b: number) => b.toString(16).padStart(2, "0"))
            .join("");
          const existing = newTransfers.find(
            (t) => t.offer.hash === hashHex
          );
          if (!existing) {
            newTransfers.push({
              offer: {
                sender_nickname: fo.nickname,
                sender_id: fo.endpoint_id,
                filename: fo.filename,
                size: fo.size,
                hash: hashHex,
              },
              state: "Pending",
            });
          }
        }
      }
      return {
        ...state,
        transfers: newTransfers,
        messages: [
          ...historyMsgs,
          {
            id: nextId(),
            kind: "system",
            text: `— synced ${merged_count} message${merged_count > 1 ? "s" : ""} from history —`,
          },
          ...state.messages,
        ],
      };
    }

    case "SYSTEM": {
      const { message } = action.payload.System;
      return {
        ...state,
        messages: [
          ...state.messages,
          { id: nextId(), kind: "system", text: message },
        ],
      };
    }

    case "DISCONNECTED": {
      const { reason } = action.payload.Disconnected;
      return {
        ...state,
        connected: false,
        messages: [
          ...state.messages,
          {
            id: nextId(),
            kind: "system",
            text: `Disconnected: ${reason}`,
          },
        ],
      };
    }

    case "CLEAR_ERROR":
      return { ...state, error: null };

    default:
      return state;
  }
}

// ── Hook ────────────────────────────────────────────────────────────────────

export function useSession() {
  const [state, dispatch] = useReducer(reducer, initialState);
  const unlistenRef = useRef<(() => void) | null>(null);

  // Register event listeners once the session starts
  const startListening = useCallback(async () => {
    // Clean up any existing listeners
    unlistenRef.current?.();

    const unlisten = await listenAll({
      onReady: (p) => dispatch({ type: "READY", payload: p }),
      onPeerJoined: (p) => dispatch({ type: "PEER_JOINED", payload: p }),
      onPeerLeft: (p) => dispatch({ type: "PEER_LEFT", payload: p }),
      onChatReceived: (p) =>
        dispatch({ type: "CHAT_RECEIVED", payload: p }),
      onChatSent: (p) => dispatch({ type: "CHAT_SENT", payload: p }),
      onFileOffered: (p) =>
        dispatch({ type: "FILE_OFFERED", payload: p }),
      onFileRetracted: (p) =>
        dispatch({ type: "FILE_RETRACTED", payload: p }),
      onFileShared: (p) => dispatch({ type: "FILE_SHARED", payload: p }),
      onFileShareFailed: (p) =>
        dispatch({ type: "FILE_SHARE_FAILED", payload: p }),
      onTransferProgress: (p) =>
        dispatch({ type: "TRANSFER_PROGRESS", payload: p }),
      onTransferComplete: (p) =>
        dispatch({ type: "TRANSFER_COMPLETE", payload: p }),
      onTransferFailed: (p) =>
        dispatch({ type: "TRANSFER_FAILED", payload: p }),
      onConnTypeChanged: (p) =>
        dispatch({ type: "CONN_TYPE_CHANGED", payload: p }),
      onHistorySynced: (p) =>
        dispatch({ type: "HISTORY_SYNCED", payload: p }),
      onSystem: (p) => dispatch({ type: "SYSTEM", payload: p }),
      onDisconnected: (p) =>
        dispatch({ type: "DISCONNECTED", payload: p }),
    });

    unlistenRef.current = unlisten;
  }, []);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  // Action creators — register listeners BEFORE invoking the command
  // so we don't miss the Ready event that fires immediately.
  const createSession = useCallback(
    async (nickname: string) => {
      await startListening();
      dispatch({ type: "SESSION_STARTED", nickname });
      await commands.createSession(nickname);
    },
    [startListening]
  );

  const joinSession = useCallback(
    async (nickname: string, ticket: string) => {
      await startListening();
      dispatch({ type: "SESSION_STARTED", nickname });
      await commands.joinSession(nickname, ticket);
    },
    [startListening]
  );

  const sendChat = useCallback(async (text: string) => {
    await commands.sendChat(text);
  }, []);

  const shareFile = useCallback(
    async (path: string, target?: string) => {
      await commands.shareFile(path, target);
    },
    []
  );

  const startDownload = useCallback(async (hash: string) => {
    await commands.startDownload(hash);
  }, []);

  const unshareFile = useCallback(async (hash: string) => {
    await commands.unshareFile(hash);
  }, []);

  const quitSession = useCallback(async () => {
    await commands.quitSession();
  }, []);

  const clearError = useCallback(() => {
    dispatch({ type: "CLEAR_ERROR" });
  }, []);

  return {
    state,
    createSession,
    joinSession,
    sendChat,
    shareFile,
    startDownload,
    unshareFile,
    quitSession,
    clearError,
  };
}
