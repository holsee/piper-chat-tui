import { useState, useCallback } from "react";
import { copyToClipboard } from "../utils/clipboard";
import type { Peer, ConnType } from "../types";

interface Props {
  peers: Map<string, Peer>;
  ticketStr: string | null;
}

const CONN_BADGES: Record<ConnType, { label: string; color: string }> = {
  You: { label: "you", color: "var(--accent)" },
  Direct: { label: "\u26A1", color: "var(--success)" },
  Relay: { label: "\u2601", color: "var(--warning)" },
  Unknown: { label: "?", color: "var(--text-dim)" },
};

export function PeersPane({ peers, ticketStr }: Props) {
  const [copied, setCopied] = useState(false);

  const copyTicket = useCallback(async () => {
    if (!ticketStr) return;
    await copyToClipboard(ticketStr);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [ticketStr]);

  const peerList = Array.from(peers.values());

  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <span style={styles.headerText}>Peers</span>
        <span style={styles.count}>{peerList.length}</span>
      </div>

      <div style={styles.list}>
        {peerList.map((peer) => {
          const badge = CONN_BADGES[peer.connType];
          return (
            <div key={peer.endpointId} style={styles.peerRow}>
              <span style={{ ...styles.badge, color: badge.color }}>
                [{badge.label}]
              </span>
              <span style={styles.peerName}>{peer.nickname}</span>
            </div>
          );
        })}
      </div>

      {ticketStr && (
        <button
          style={styles.copyBtn}
          onClick={copyTicket}
        >
          {copied ? "Copied!" : "Copy Ticket"}
        </button>
      )}
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    width: 180,
    borderLeft: "1px solid var(--border)",
    background: "var(--bg-secondary)",
    display: "flex",
    flexDirection: "column",
    flexShrink: 0,
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "0.65rem 0.75rem",
    borderBottom: "1px solid var(--border)",
  },
  headerText: {
    fontSize: "0.8rem",
    fontWeight: 600,
    color: "var(--text-muted)",
    textTransform: "uppercase" as const,
    letterSpacing: "0.04em",
  },
  count: {
    fontSize: "0.7rem",
    padding: "0.1rem 0.4rem",
    background: "var(--bg-tertiary)",
    color: "var(--text-muted)",
    borderRadius: 8,
  },
  list: {
    flex: 1,
    overflowY: "auto",
    padding: "0.5rem 0",
  },
  peerRow: {
    display: "flex",
    alignItems: "center",
    gap: "0.35rem",
    padding: "0.3rem 0.75rem",
    fontSize: "0.85rem",
  },
  badge: {
    fontSize: "0.75rem",
    fontWeight: 500,
    flexShrink: 0,
  },
  peerName: {
    color: "var(--text)",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  copyBtn: {
    margin: "0.5rem 0.75rem 0.75rem",
    padding: "0.45rem",
    background: "var(--accent)",
    color: "#fff",
    borderRadius: 6,
    fontSize: "0.8rem",
    fontWeight: 600,
    transition: "background 0.15s",
  },
};
