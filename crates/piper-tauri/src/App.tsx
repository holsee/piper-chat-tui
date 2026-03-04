import { useState, useEffect, useCallback } from "react";
import { copyToClipboard } from "./utils/clipboard";
import { useSession } from "./hooks/useSession";
import { WelcomeScreen } from "./components/WelcomeScreen";
import { ChatView } from "./components/ChatView";
import {
  darkTheme,
  lightTheme,
  applyTheme,
  getStoredTheme,
  storeTheme,
} from "./styles/theme";

export default function App() {
  const session = useSession();
  const [theme, setTheme] = useState<"dark" | "light">(getStoredTheme);

  // Apply theme on mount and change
  useEffect(() => {
    applyTheme(theme === "dark" ? darkTheme : lightTheme);
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme((t) => {
      const next = t === "dark" ? "light" : "dark";
      storeTheme(next);
      return next;
    });
  }, []);

  // Global keyboard shortcuts
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "t") {
        e.preventDefault();
        toggleTheme();
      }
      if (e.ctrlKey && e.key === "y") {
        e.preventDefault();
        const ticket = session.state.ticketStr;
        if (ticket) {
          copyToClipboard(ticket);
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [toggleTheme, session.state.ticketStr]);

  if (session.state.screen === "welcome") {
    return (
      <WelcomeScreen
        onCreateSession={session.createSession}
        onJoinSession={session.joinSession}
        theme={theme}
        onToggleTheme={toggleTheme}
      />
    );
  }

  return (
    <ChatView
      session={session}
      theme={theme}
      onToggleTheme={toggleTheme}
    />
  );
}
