import { invoke, Channel } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open } from "@tauri-apps/plugin-dialog";

export type Progress =
  | { phase: "preparing"; pct: number }
  | { phase: "decoding" }
  | { phase: "transcribing"; pct: number }
  | { phase: "rendering" };

export interface Outcome {
  docx: string;
  pdf: string;
  backend: string;
  durationSecs: number;
  paragraphs: number;
}

/**
 * The shape `transcribe_file`/`cancel_job` reject with on failure (see
 * `CommandError` in `app/src-tauri/src/commands.rs`). `kind` lets the caller
 * tell a user-requested cancellation apart from every other failure without
 * matching on English message text. Tauri rejects the invoke promise with this
 * object as-is (deserialized from the command's JSON `Err`), not with a string.
 */
export interface CommandError {
  kind: "cancelled" | "failed";
  message: string;
}

const AUDIO_EXTENSIONS = ["mp3", "m4a", "mp4", "wav", "flac", "ogg", "opus", "mpeg", "mpga", "aac"];

export function looksLikeAudio(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return AUDIO_EXTENSIONS.includes(ext);
}

export async function pickFile(): Promise<string | null> {
  const picked = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "Áudio", extensions: AUDIO_EXTENSIONS }],
  });
  return typeof picked === "string" ? picked : null;
}

/**
 * OS file drops never reach DOM drag events in Tauri — the native window layer
 * intercepts them first, and `dragDropEnabled` actively disables DOM drag/drop.
 * This webview event is the only way to receive dropped paths.
 */
export async function onFilesDropped(handler: (paths: string[]) => void): Promise<void> {
  await getCurrentWebviewWindow().onDragDropEvent((event) => {
    if (event.payload.type === "drop" && event.payload.paths.length > 0) {
      handler(event.payload.paths);
    }
  });
}

export async function transcribe(
  path: string,
  language: string,
  jobId: string,
  onProgress: (p: Progress) => void,
): Promise<Outcome> {
  const channel = new Channel<Progress>();
  channel.onmessage = onProgress;
  return invoke<Outcome>("transcribe_file", { path, language, jobId, progress: channel });
}

export async function cancel(jobId: string): Promise<void> {
  await invoke("cancel_job", { jobId });
}
