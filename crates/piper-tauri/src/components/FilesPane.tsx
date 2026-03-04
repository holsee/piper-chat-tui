import { useCallback } from "react";
import { open } from "@tauri-apps/plugin-shell";
import type { TransferEntry } from "../types";
import { formatFileSize } from "../hooks/useSession";

interface Props {
  transfers: TransferEntry[];
  onDownload: (hash: string) => Promise<void>;
  onUnshare: (hash: string) => Promise<void>;
}

export function FilesPane({ transfers, onDownload, onUnshare }: Props) {
  return (
    <div style={styles.container}>
      <div style={styles.header}>
        <span style={styles.headerText}>Files</span>
        <span style={styles.count}>{transfers.length}</span>
      </div>
      <div style={styles.list}>
        {transfers.map((entry) => (
          <FileRow
            key={entry.offer.hash}
            entry={entry}
            onDownload={onDownload}
            onUnshare={onUnshare}
          />
        ))}
      </div>
    </div>
  );
}

function FileRow({
  entry,
  onDownload,
  onUnshare,
}: {
  entry: TransferEntry;
  onDownload: (hash: string) => Promise<void>;
  onUnshare: (hash: string) => Promise<void>;
}) {
  const { offer, state } = entry;

  const handleDownload = useCallback(() => {
    onDownload(offer.hash);
  }, [offer.hash, onDownload]);

  const handleUnshare = useCallback(() => {
    onUnshare(offer.hash);
  }, [offer.hash, onUnshare]);

  const handleOpen = useCallback(async () => {
    if (typeof state === "object" && "Complete" in state) {
      // Open the containing folder
      const path = state.Complete;
      const dir = path.substring(0, path.lastIndexOf("/")) || path.substring(0, path.lastIndexOf("\\"));
      try {
        await open(dir || path);
      } catch {
        // fallback: open the file itself
        try { await open(path); } catch { /* ignore */ }
      }
    }
  }, [state]);

  // Determine action button
  let actionEl: React.ReactNode = null;
  let statusEl: React.ReactNode = null;

  if (state === "Pending") {
    actionEl = (
      <button style={styles.actionBtn} onClick={handleDownload}>
        Download
      </button>
    );
  } else if (state === "Sharing") {
    actionEl = (
      <button style={styles.unshareBtnStyle} onClick={handleUnshare}>
        Unshare
      </button>
    );
    statusEl = <span style={styles.sharingLabel}>sharing</span>;
  } else if (typeof state === "object" && "Downloading" in state) {
    const { bytes_received, total_bytes } = state.Downloading;
    const pct = total_bytes > 0 ? (bytes_received / total_bytes) * 100 : 0;
    statusEl = (
      <div style={styles.progressContainer}>
        <div style={styles.progressTrack}>
          <div
            style={{ ...styles.progressBar, width: `${pct}%` }}
          />
        </div>
        <span style={styles.progressText}>{Math.round(pct)}%</span>
      </div>
    );
  } else if (typeof state === "object" && "Complete" in state) {
    actionEl = (
      <button style={styles.openBtn} onClick={handleOpen}>
        Open
      </button>
    );
    statusEl = <span style={styles.completeLabel}>done</span>;
  } else if (typeof state === "object" && "Failed" in state) {
    statusEl = (
      <span style={styles.failedLabel} title={state.Failed}>
        failed
      </span>
    );
  }

  return (
    <div style={styles.row}>
      <div style={styles.fileInfo}>
        <span style={styles.filename}>{offer.filename}</span>
        <span style={styles.fileSize}>
          {formatFileSize(offer.size)}
          {offer.sender_nickname !== "You" && (
            <> from {offer.sender_nickname}</>
          )}
        </span>
      </div>
      <div style={styles.fileActions}>
        {statusEl}
        {actionEl}
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    borderTop: "1px solid var(--border)",
    background: "var(--bg-secondary)",
    maxHeight: "30%",
    display: "flex",
    flexDirection: "column",
    flexShrink: 0,
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "0.4rem 0.75rem",
    borderBottom: "1px solid var(--border)",
  },
  headerText: {
    fontSize: "0.75rem",
    fontWeight: 600,
    color: "var(--text-muted)",
    textTransform: "uppercase" as const,
    letterSpacing: "0.04em",
  },
  count: {
    fontSize: "0.65rem",
    padding: "0.1rem 0.35rem",
    background: "var(--bg-tertiary)",
    color: "var(--text-muted)",
    borderRadius: 8,
  },
  list: {
    overflowY: "auto",
    padding: "0.25rem 0",
  },
  row: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "0.35rem 0.75rem",
    gap: "0.5rem",
  },
  fileInfo: {
    display: "flex",
    flexDirection: "column",
    minWidth: 0,
  },
  filename: {
    fontSize: "0.85rem",
    color: "var(--text)",
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  fileSize: {
    fontSize: "0.72rem",
    color: "var(--text-dim)",
  },
  fileActions: {
    display: "flex",
    alignItems: "center",
    gap: "0.4rem",
    flexShrink: 0,
  },
  actionBtn: {
    padding: "0.25rem 0.6rem",
    background: "var(--accent)",
    color: "#fff",
    borderRadius: 4,
    fontSize: "0.75rem",
    fontWeight: 600,
  },
  unshareBtnStyle: {
    padding: "0.25rem 0.6rem",
    background: "var(--error)",
    color: "#fff",
    borderRadius: 4,
    fontSize: "0.75rem",
    fontWeight: 600,
  },
  openBtn: {
    padding: "0.25rem 0.6rem",
    background: "var(--success)",
    color: "#fff",
    borderRadius: 4,
    fontSize: "0.75rem",
    fontWeight: 600,
  },
  sharingLabel: {
    fontSize: "0.7rem",
    color: "var(--accent)",
    fontWeight: 500,
  },
  completeLabel: {
    fontSize: "0.7rem",
    color: "var(--success)",
    fontWeight: 500,
  },
  failedLabel: {
    fontSize: "0.7rem",
    color: "var(--error)",
    fontWeight: 500,
  },
  progressContainer: {
    display: "flex",
    alignItems: "center",
    gap: "0.35rem",
  },
  progressTrack: {
    width: 60,
    height: 6,
    background: "var(--bg-tertiary)",
    borderRadius: 3,
    overflow: "hidden",
  },
  progressBar: {
    height: "100%",
    background: "var(--accent)",
    borderRadius: 3,
    transition: "width 0.15s",
  },
  progressText: {
    fontSize: "0.7rem",
    color: "var(--text-muted)",
    width: "2.5rem",
    textAlign: "right" as const,
  },
};
