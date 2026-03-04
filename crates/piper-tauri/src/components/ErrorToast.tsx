import { useEffect } from "react";

interface Props {
  message: string;
  onDismiss: () => void;
}

export function ErrorToast({ message, onDismiss }: Props) {
  useEffect(() => {
    const timer = setTimeout(onDismiss, 5000);
    return () => clearTimeout(timer);
  }, [message, onDismiss]);

  return (
    <div style={styles.container} onClick={onDismiss}>
      <span style={styles.text}>{message}</span>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  container: {
    position: "fixed",
    bottom: 16,
    left: "50%",
    transform: "translateX(-50%)",
    padding: "0.5rem 1rem",
    background: "var(--error)",
    color: "#fff",
    borderRadius: 8,
    fontSize: "0.85rem",
    fontWeight: 500,
    boxShadow: "0 4px 12px rgba(0,0,0,0.3)",
    cursor: "pointer",
    zIndex: 100,
    maxWidth: "80%",
    textAlign: "center",
  },
  text: {},
};
