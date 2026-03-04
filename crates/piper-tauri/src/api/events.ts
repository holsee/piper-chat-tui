import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
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

type Handler<T> = (payload: T) => void;

export interface EventListeners {
  onReady?: Handler<ReadyPayload>;
  onPeerJoined?: Handler<PeerJoinedPayload>;
  onPeerLeft?: Handler<PeerLeftPayload>;
  onChatReceived?: Handler<ChatReceivedPayload>;
  onChatSent?: Handler<ChatSentPayload>;
  onFileOffered?: Handler<FileOfferedPayload>;
  onFileRetracted?: Handler<FileRetractedPayload>;
  onFileShared?: Handler<FileSharedPayload>;
  onFileShareFailed?: Handler<FileShareFailedPayload>;
  onTransferProgress?: Handler<TransferProgressPayload>;
  onTransferComplete?: Handler<TransferCompletePayload>;
  onTransferFailed?: Handler<TransferFailedPayload>;
  onConnTypeChanged?: Handler<ConnTypeChangedPayload>;
  onHistorySynced?: Handler<HistorySyncedPayload>;
  onSystem?: Handler<SystemPayload>;
  onDisconnected?: Handler<DisconnectedPayload>;
}

const EVENT_MAP: Array<[string, keyof EventListeners]> = [
  ["session:ready", "onReady"],
  ["session:peer-joined", "onPeerJoined"],
  ["session:peer-left", "onPeerLeft"],
  ["session:chat-received", "onChatReceived"],
  ["session:chat-sent", "onChatSent"],
  ["session:file-offered", "onFileOffered"],
  ["session:file-retracted", "onFileRetracted"],
  ["session:file-shared", "onFileShared"],
  ["session:file-share-failed", "onFileShareFailed"],
  ["session:transfer-progress", "onTransferProgress"],
  ["session:transfer-complete", "onTransferComplete"],
  ["session:transfer-failed", "onTransferFailed"],
  ["session:conn-type-changed", "onConnTypeChanged"],
  ["session:history-synced", "onHistorySynced"],
  ["session:system", "onSystem"],
  ["session:disconnected", "onDisconnected"],
];

/** Register all session event listeners. Returns an unlisten function. */
export async function listenAll(
  handlers: EventListeners
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  for (const [eventName, handlerKey] of EVENT_MAP) {
    const handler = handlers[handlerKey];
    if (handler) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const fn = handler as (payload: any) => void;
      const unlisten = await listen(eventName, (event) => {
        fn(event.payload);
      });
      unlisteners.push(unlisten);
    }
  }

  return () => {
    for (const unlisten of unlisteners) {
      unlisten();
    }
  };
}
