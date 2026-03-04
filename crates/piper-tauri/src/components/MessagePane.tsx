import { useRef, useEffect, useState, useCallback } from "react";
import { copyToClipboard } from "../utils/clipboard";
import type { ChatMessage } from "../types";

interface Props {
  messages: ChatMessage[];
  connected: boolean;
}

export function MessagePane({ messages, connected }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  const [showNewIndicator, setShowNewIndicator] = useState(false);
  const prevLengthRef = useRef(messages.length);

  // Detect manual scroll
  const handleScroll = useCallback(() => {
    const el = containerRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    setAutoScroll(atBottom);
    if (atBottom) setShowNewIndicator(false);
  }, []);

  // Auto-scroll on new messages
  useEffect(() => {
    if (messages.length > prevLengthRef.current && !autoScroll) {
      setShowNewIndicator(true);
    }
    prevLengthRef.current = messages.length;

    if (autoScroll && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [messages, autoScroll]);

  const scrollToBottom = useCallback(() => {
    if (containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
      setAutoScroll(true);
      setShowNewIndicator(false);
    }
  }, []);

  return (
    <div style={styles.wrapper}>
      {!connected && (
        <div style={styles.disconnectBanner}>Disconnected</div>
      )}
      <div
        ref={containerRef}
        style={styles.container}
        onScroll={handleScroll}
      >
        {messages.map((msg) => (
          <MessageRow key={msg.id} msg={msg} />
        ))}
      </div>
      {showNewIndicator && (
        <button style={styles.newMsgBtn} onClick={scrollToBottom}>
          New messages
        </button>
      )}
    </div>
  );
}

function MessageRow({ msg }: { msg: ChatMessage }) {
  const time = msg.timestamp
    ? new Date(msg.timestamp).toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
      })
    : null;

  if (msg.kind === "system") {
    return (
      <div style={styles.systemMsg}>
        {time && <span style={styles.time}>{time} </span>}
        <span style={styles.systemText}>{msg.text}</span>
      </div>
    );
  }

  if (msg.kind === "ticket") {
    return <TicketRow text={msg.text} />;
  }

  if (msg.kind === "file") {
    return (
      <div style={styles.msg}>
        {time && <span style={styles.time}>{time} </span>}
        <span style={styles.nick}>{msg.nickname}</span>
        <span style={styles.fileText}> {msg.text}</span>
      </div>
    );
  }

  // chat
  return (
    <div style={styles.msg}>
      {time && <span style={styles.time}>{time} </span>}
      <span style={styles.nick}>{msg.nickname}</span>
      <span style={styles.chatText}>: {msg.text}</span>
    </div>
  );
}

function TicketRow({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    await copyToClipboard(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [text]);

  return (
    <div style={styles.ticketMsg}>
      <span style={styles.ticketLabel}>Ticket: </span>
      <code style={styles.ticketCode}>{text}</code>
      <button style={styles.copyBtn} onClick={handleCopy}>
        {copied ? "Copied!" : "Copy"}
      </button>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrapper: {
    flex: 1,
    position: "relative",
    overflow: "hidden",
    display: "flex",
    flexDirection: "column",
  },
  disconnectBanner: {
    padding: "0.35rem 0.75rem",
    background: "var(--error)",
    color: "#fff",
    fontSize: "0.8rem",
    fontWeight: 600,
    textAlign: "center",
  },
  container: {
    flex: 1,
    overflowY: "auto",
    padding: "0.75rem 1rem",
    display: "flex",
    flexDirection: "column",
    gap: "0.15rem",
  },
  msg: {
    padding: "0.2rem 0",
    fontSize: "0.9rem",
    lineHeight: 1.45,
    wordBreak: "break-word",
  },
  time: {
    color: "var(--text-dim)",
    fontSize: "0.8rem",
    marginRight: "0.25rem",
  },
  nick: {
    fontWeight: 600,
    color: "var(--accent)",
  },
  chatText: {
    color: "var(--text)",
  },
  fileText: {
    color: "var(--text-muted)",
    fontStyle: "italic",
  },
  systemMsg: {
    padding: "0.15rem 0",
    fontSize: "0.82rem",
  },
  systemText: {
    color: "var(--text-dim)",
    fontStyle: "italic",
  },
  ticketMsg: {
    padding: "0.35rem 0.6rem",
    margin: "0.25rem 0",
    background: "var(--bg-tertiary)",
    borderRadius: 6,
    fontSize: "0.82rem",
    display: "flex",
    alignItems: "center",
    gap: "0.35rem",
    flexWrap: "wrap",
  },
  ticketLabel: {
    color: "var(--text-muted)",
    fontWeight: 500,
  },
  ticketCode: {
    color: "var(--accent)",
    fontSize: "0.78rem",
    wordBreak: "break-all",
    userSelect: "all",
    flex: 1,
    minWidth: 0,
  },
  copyBtn: {
    padding: "0.2rem 0.5rem",
    background: "var(--accent)",
    color: "#fff",
    borderRadius: 4,
    fontSize: "0.72rem",
    fontWeight: 600,
    flexShrink: 0,
    cursor: "pointer",
    border: "none",
  },
  newMsgBtn: {
    position: "absolute",
    bottom: 8,
    left: "50%",
    transform: "translateX(-50%)",
    padding: "0.3rem 0.75rem",
    background: "var(--accent)",
    color: "#fff",
    borderRadius: 12,
    fontSize: "0.75rem",
    fontWeight: 600,
    boxShadow: "0 2px 8px rgba(0,0,0,0.3)",
    cursor: "pointer",
    border: "none",
  },
};
