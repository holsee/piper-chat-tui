export interface ThemeTokens {
  bg: string;
  bgSecondary: string;
  bgTertiary: string;
  text: string;
  textMuted: string;
  textDim: string;
  accent: string;
  accentHover: string;
  accentMuted: string;
  border: string;
  borderFocus: string;
  success: string;
  error: string;
  warning: string;
  inputBg: string;
  scrollbarThumb: string;
}

export const darkTheme: ThemeTokens = {
  bg: "#0f0f14",
  bgSecondary: "#1a1a24",
  bgTertiary: "#24243a",
  text: "#e2e2f0",
  textMuted: "#8888a8",
  textDim: "#555570",
  accent: "#8b5cf6",
  accentHover: "#a78bfa",
  accentMuted: "#6d28d9",
  border: "#2a2a40",
  borderFocus: "#8b5cf6",
  success: "#22c55e",
  error: "#ef4444",
  warning: "#f59e0b",
  inputBg: "#14141e",
  scrollbarThumb: "#3a3a55",
};

export const lightTheme: ThemeTokens = {
  bg: "#fafafa",
  bgSecondary: "#f0f0f5",
  bgTertiary: "#e5e5f0",
  text: "#1a1a2e",
  textMuted: "#6b6b8a",
  textDim: "#9999b0",
  accent: "#7c3aed",
  accentHover: "#6d28d9",
  accentMuted: "#a78bfa",
  border: "#d5d5e0",
  borderFocus: "#7c3aed",
  success: "#16a34a",
  error: "#dc2626",
  warning: "#d97706",
  inputBg: "#ffffff",
  scrollbarThumb: "#c5c5d5",
};

export function applyTheme(theme: ThemeTokens) {
  const root = document.documentElement;
  for (const [key, value] of Object.entries(theme)) {
    root.style.setProperty(`--${camelToKebab(key)}`, value);
  }
}

function camelToKebab(s: string): string {
  return s.replace(/([A-Z])/g, "-$1").toLowerCase();
}

export function getStoredTheme(): "dark" | "light" {
  return (localStorage.getItem("piper-theme") as "dark" | "light") ?? "dark";
}

export function storeTheme(theme: "dark" | "light") {
  localStorage.setItem("piper-theme", theme);
}
