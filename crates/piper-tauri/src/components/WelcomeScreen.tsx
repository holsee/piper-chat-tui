import { useState, useCallback, type FormEvent, type KeyboardEvent } from "react";

interface Props {
  onCreateSession: (nickname: string) => Promise<void>;
  onJoinSession: (nickname: string, ticket: string) => Promise<void>;
  theme: "dark" | "light";
  onToggleTheme: () => void;
}

export function WelcomeScreen({
  onCreateSession,
  onJoinSession,
  theme,
  onToggleTheme,
}: Props) {
  const [nickname, setNickname] = useState("");
  const [ticket, setTicket] = useState("");
  const [mode, setMode] = useState<"create" | "join">("create");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = useCallback(
    async (e?: FormEvent) => {
      e?.preventDefault();
      const name = nickname.trim();
      if (!name) {
        setError("Nickname is required");
        return;
      }
      if (mode === "join" && !ticket.trim()) {
        setError("Ticket is required to join");
        return;
      }

      setLoading(true);
      setError(null);
      try {
        if (mode === "create") {
          await onCreateSession(name);
        } else {
          await onJoinSession(name, ticket.trim());
        }
      } catch (err) {
        setError(String(err));
        setLoading(false);
      }
    },
    [nickname, ticket, mode, onCreateSession, onJoinSession]
  );

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === "Enter" && !loading) {
        handleSubmit();
      }
    },
    [handleSubmit, loading]
  );

  return (
    <div style={styles.container}>
      <div style={styles.card}>
        <div style={styles.logo}>
          <span style={styles.logoIcon}>&#9830;</span>
          <h1 style={styles.title}>piper-chat</h1>
        </div>
        <p style={styles.subtitle}>
          Peer-to-peer encrypted chat over iroh
        </p>

        <div style={styles.modeToggle}>
          <button
            style={{
              ...styles.modeBtn,
              ...(mode === "create" ? styles.modeBtnActive : {}),
            }}
            onClick={() => setMode("create")}
          >
            Create Room
          </button>
          <button
            style={{
              ...styles.modeBtn,
              ...(mode === "join" ? styles.modeBtnActive : {}),
            }}
            onClick={() => setMode("join")}
          >
            Join Room
          </button>
        </div>

        <form onSubmit={handleSubmit} style={styles.form}>
          <div style={styles.field}>
            <label style={styles.label}>Nickname</label>
            <input
              style={styles.input}
              value={nickname}
              onChange={(e) => setNickname(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Enter your name"
              autoFocus
              maxLength={32}
            />
          </div>

          {mode === "join" && (
            <div style={styles.field}>
              <label style={styles.label}>Ticket</label>
              <input
                style={styles.input}
                value={ticket}
                onChange={(e) => setTicket(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder="Paste the room ticket"
              />
            </div>
          )}

          {error && <div style={styles.error}>{error}</div>}

          <button
            type="submit"
            style={{
              ...styles.submit,
              opacity: loading ? 0.6 : 1,
            }}
            disabled={loading}
          >
            {loading
              ? "Connecting..."
              : mode === "create"
                ? "Create Room"
                : "Join Room"}
          </button>
        </form>

        <button style={styles.themeBtn} onClick={onToggleTheme}>
          {theme === "dark" ? "Light Mode" : "Dark Mode"}
        </button>
      </div>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: "100%",
    padding: "2rem",
  },
  card: {
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    gap: "1.25rem",
    width: "100%",
    maxWidth: 380,
    padding: "2.5rem 2rem",
    background: "var(--bg-secondary)",
    borderRadius: 16,
    border: "1px solid var(--border)",
  },
  logo: {
    display: "flex",
    alignItems: "center",
    gap: "0.5rem",
  },
  logoIcon: {
    fontSize: "1.8rem",
    color: "var(--accent)",
  },
  title: {
    fontSize: "1.6rem",
    fontWeight: 700,
    color: "var(--text)",
    letterSpacing: "-0.02em",
  },
  subtitle: {
    fontSize: "0.85rem",
    color: "var(--text-muted)",
    marginTop: "-0.5rem",
  },
  modeToggle: {
    display: "flex",
    gap: "0.25rem",
    padding: "0.2rem",
    background: "var(--bg-tertiary)",
    borderRadius: 8,
    width: "100%",
  },
  modeBtn: {
    flex: 1,
    padding: "0.5rem",
    borderRadius: 6,
    background: "transparent",
    color: "var(--text-muted)",
    fontSize: "0.85rem",
    fontWeight: 500,
    transition: "all 0.15s",
  },
  modeBtnActive: {
    background: "var(--accent)",
    color: "#fff",
  },
  form: {
    display: "flex",
    flexDirection: "column",
    gap: "1rem",
    width: "100%",
  },
  field: {
    display: "flex",
    flexDirection: "column",
    gap: "0.35rem",
  },
  label: {
    fontSize: "0.8rem",
    fontWeight: 500,
    color: "var(--text-muted)",
    textTransform: "uppercase" as const,
    letterSpacing: "0.04em",
  },
  input: {
    padding: "0.65rem 0.85rem",
    background: "var(--input-bg)",
    color: "var(--text)",
    border: "1px solid var(--border)",
    borderRadius: 8,
    fontSize: "0.95rem",
    transition: "border-color 0.15s",
  },
  error: {
    padding: "0.5rem 0.75rem",
    background: "rgba(239, 68, 68, 0.1)",
    color: "var(--error)",
    borderRadius: 6,
    fontSize: "0.85rem",
  },
  submit: {
    padding: "0.7rem",
    background: "var(--accent)",
    color: "#fff",
    borderRadius: 8,
    fontSize: "0.95rem",
    fontWeight: 600,
    transition: "background 0.15s, opacity 0.15s",
  },
  themeBtn: {
    padding: "0.4rem 0.8rem",
    background: "transparent",
    color: "var(--text-dim)",
    fontSize: "0.75rem",
    borderRadius: 6,
    border: "1px solid var(--border)",
  },
};
