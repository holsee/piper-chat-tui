import {
  useState,
  useCallback,
  useRef,
  type KeyboardEvent,
} from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

interface Props {
  onSend: (text: string) => Promise<void>;
  onShareFile: (path: string, target?: string) => Promise<void>;
  disabled: boolean;
  nickname: string;
  theme: "dark" | "light";
  onToggleTheme: () => void;
}

const HELP_TEXT = [
  "/help — show this help",
  "/send — pick a file to share with everyone",
  "/sendto <name> — share a file with a specific peer",
  "Ctrl+F — pick a file to share",
  "Ctrl+T — toggle dark/light theme",
  "Ctrl+Y — copy ticket to clipboard",
].join("\n");

export function ChatInput({
  onSend,
  onShareFile,
  disabled,
  theme: _theme,
  onToggleTheme: _onToggleTheme,
}: Props) {
  const [text, setText] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const handleSend = useCallback(async () => {
    const trimmed = text.trim();
    if (!trimmed) return;

    // Handle slash commands
    if (trimmed === "/help") {
      setText("");
      // Show help as an alert for now
      alert(HELP_TEXT);
      return;
    }

    if (trimmed === "/send") {
      setText("");
      await pickAndShare();
      return;
    }

    if (trimmed.startsWith("/sendto ")) {
      const target = trimmed.slice(8).trim();
      if (target) {
        setText("");
        await pickAndShare(target);
      }
      return;
    }

    setText("");
    try {
      await onSend(trimmed);
    } catch {
      // Error will be shown via error toast
    }
  }, [text, onSend]);

  const pickAndShare = useCallback(
    async (target?: string) => {
      try {
        const result = await openDialog({
          multiple: false,
          directory: false,
        });
        if (result) {
          await onShareFile(result, target);
        }
      } catch {
        // User cancelled or error
      }
    },
    [onShareFile]
  );

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
      // Ctrl+F: file picker
      if (e.ctrlKey && e.key === "f") {
        e.preventDefault();
        pickAndShare();
      }
    },
    [handleSend, pickAndShare]
  );

  return (
    <div style={styles.container}>
      <input
        ref={inputRef}
        style={{
          ...styles.input,
          opacity: disabled ? 0.5 : 1,
        }}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={disabled ? "Disconnected" : "Type a message..."}
        disabled={disabled}
        autoFocus
      />
      <button
        style={styles.shareBtn}
        onClick={() => pickAndShare()}
        disabled={disabled}
        title="Share file (Ctrl+F)"
      >
        Share
      </button>
      <button
        style={styles.sendBtn}
        onClick={handleSend}
        disabled={disabled || !text.trim()}
      >
        &#9166;
      </button>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    display: "flex",
    alignItems: "center",
    gap: "0.4rem",
    padding: "0.6rem 0.75rem",
    borderTop: "1px solid var(--border)",
    background: "var(--bg-secondary)",
  },
  input: {
    flex: 1,
    padding: "0.55rem 0.75rem",
    background: "var(--input-bg)",
    color: "var(--text)",
    border: "1px solid var(--border)",
    borderRadius: 8,
    fontSize: "0.9rem",
    transition: "border-color 0.15s, opacity 0.15s",
  },
  shareBtn: {
    padding: "0.5rem 0.65rem",
    background: "var(--bg-tertiary)",
    color: "var(--text-muted)",
    borderRadius: 6,
    fontSize: "0.8rem",
    fontWeight: 500,
    border: "1px solid var(--border)",
    flexShrink: 0,
  },
  sendBtn: {
    padding: "0.5rem 0.7rem",
    background: "var(--accent)",
    color: "#fff",
    borderRadius: 6,
    fontSize: "1rem",
    fontWeight: 600,
    flexShrink: 0,
  },
};
