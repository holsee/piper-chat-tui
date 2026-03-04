import { invoke } from "@tauri-apps/api/core";

export async function createSession(nickname: string): Promise<string> {
  return invoke<string>("create_session", { nickname });
}

export async function joinSession(
  nickname: string,
  ticketStr: string
): Promise<string> {
  return invoke<string>("join_session", { nickname, ticketStr });
}

export async function sendChat(text: string): Promise<void> {
  return invoke("send_chat", { text });
}

export async function shareFile(
  path: string,
  target?: string
): Promise<void> {
  return invoke("share_file", { path, target: target ?? null });
}

export async function startDownload(hash: string): Promise<void> {
  return invoke("start_download", { hash });
}

export async function unshareFile(hash: string): Promise<void> {
  return invoke("unshare_file", { hash });
}

export async function quitSession(): Promise<void> {
  return invoke("quit_session");
}
