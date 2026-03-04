import { useEffect } from "react";
import type { useSession } from "../hooks/useSession";
import { MessagePane } from "./MessagePane";
import { PeersPane } from "./PeersPane";
import { FilesPane } from "./FilesPane";
import { ChatInput } from "./ChatInput";
import { ErrorToast } from "./ErrorToast";

interface Props {
  session: ReturnType<typeof useSession>;
  theme: "dark" | "light";
  onToggleTheme: () => void;
}

export function ChatView({ session, theme, onToggleTheme }: Props) {
  const { state } = session;
  const hasFiles = state.transfers.length > 0;
  const peerCount = state.peers.size;

  // Update window title with peer count
  useEffect(() => {
    document.title =
      peerCount > 0 ? `piper-chat (${peerCount} peer${peerCount > 1 ? "s" : ""})` : "piper-chat";
  }, [peerCount]);

  return (
    <div style={styles.container}>
      <div style={styles.main}>
        <div style={styles.messagesArea}>
          <MessagePane messages={state.messages} connected={state.connected} />
          {hasFiles && (
            <FilesPane
              transfers={state.transfers}
              onDownload={session.startDownload}
              onUnshare={session.unshareFile}
            />
          )}
        </div>
        <ChatInput
          onSend={session.sendChat}
          onShareFile={session.shareFile}
          disabled={!state.connected}
          nickname={state.nickname}
          theme={theme}
          onToggleTheme={onToggleTheme}
        />
      </div>
      <PeersPane
        peers={state.peers}
        ticketStr={state.ticketStr}
      />
      {state.error && (
        <ErrorToast message={state.error} onDismiss={session.clearError} />
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    display: "flex",
    height: "100%",
    overflow: "hidden",
  },
  main: {
    flex: 1,
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  messagesArea: {
    flex: 1,
    display: "flex",
    flexDirection: "column",
    overflow: "hidden",
  },
};
