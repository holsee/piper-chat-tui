/**
 * Copy text to clipboard with multiple fallback strategies.
 * Tauri plugin → navigator.clipboard → execCommand fallback.
 */
export async function copyToClipboard(text: string): Promise<void> {
  // 1. Try Tauri clipboard plugin
  try {
    const { writeText } = await import(
      "@tauri-apps/plugin-clipboard-manager"
    );
    await writeText(text);
    return;
  } catch {
    // plugin unavailable or failed
  }

  // 2. Try navigator.clipboard (requires secure context)
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    // not available or denied
  }

  // 3. Fallback: hidden textarea + execCommand
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.left = "-9999px";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    document.execCommand("copy");
  } finally {
    document.body.removeChild(textarea);
  }
}
