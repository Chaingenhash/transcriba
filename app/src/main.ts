import {
  cancel,
  looksLikeAudio,
  onFilesDropped,
  pickFile,
  transcribe,
  type CommandError,
  type Outcome,
  type Progress,
} from "./api";

const el = <T extends HTMLElement>(sel: string): T => document.querySelector<T>(sel)!;
const drop = el<HTMLDivElement>("#drop");
const pick = el<HTMLButtonElement>("#pick");
const lang = el<HTMLSelectElement>("#lang");
const status = el<HTMLElement>("#status");
const phase = el<HTMLParagraphElement>("#phase");
const bar = el<HTMLProgressElement>("#bar");
const cancelBtn = el<HTMLButtonElement>("#cancel");
const result = el<HTMLElement>("#result");

const LANGUAGES: [string, string][] = [
  ["pt", "Português"],
  ["auto", "Detetar automaticamente"],
  ["en", "English"],
  ["es", "Español"],
  ["fr", "Français"],
  ["de", "Deutsch"],
  ["it", "Italiano"],
];

for (const [code, label] of LANGUAGES) {
  const opt = document.createElement("option");
  opt.value = code;
  opt.textContent = label;
  lang.append(opt);
}
lang.value = "pt";

let running: string | null = null;

function describe(p: Progress): { text: string; value: number } {
  switch (p.phase) {
    case "preparing":
      return { text: `A preparar o modelo de voz… ${p.pct}%`, value: Math.min(5, p.pct / 20) };
    case "decoding":
      return { text: "A ler o áudio…", value: 5 };
    case "transcribing":
      return { text: `A transcrever… ${p.pct}%`, value: 5 + (p.pct * 90) / 100 };
    case "rendering":
      return { text: "A criar os documentos…", value: 95 };
  }
}

function showError(message: string) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result error";
  result.textContent = `Erro: ${message}`;
}

// Cancellation is not an error: no red styling, no "Erro:" prefix, a plain
// Portuguese sentence in the same result panel other outcomes use.
function showCancelled() {
  status.hidden = true;
  result.hidden = false;
  result.className = "result";
  result.textContent = "Cancelado.";
}

/**
 * `o.docx`/`o.pdf` are filesystem paths derived from whatever the user dropped.
 * `<` and `>` are legal in Linux filenames, so a file named e.g.
 * `meet<img src=x onerror=...>.mp3` must not be interpolated into innerHTML —
 * that would inject and execute arbitrary HTML (there is no CSP mitigation for
 * markup assigned this way; the CSP added in tauri.conf.json blocks scripts and
 * external resources, not inline HTML assigned via the DOM). Build elements and
 * assign text via `textContent`/`append`, exactly like `showError` above.
 */
function showDone(o: Outcome) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result ok";
  result.replaceChildren();

  const mins = Math.round(o.durationSecs / 60);
  const summary = document.createElement("p");
  const strong = document.createElement("strong");
  strong.textContent = "Concluído.";
  summary.append(strong, ` ${mins} minutos de áudio, ${o.paragraphs} parágrafos.`);

  const backend = document.createElement("p");
  backend.textContent = `Transcrito com ${o.backend}.`;

  const paths = document.createElement("p");
  paths.className = "paths";
  paths.append(o.docx, document.createElement("br"), o.pdf);

  result.append(summary, backend, paths);
}

function isCommandError(e: unknown): e is CommandError {
  return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

function errorMessage(e: unknown): string {
  return isCommandError(e) ? e.message : String(e);
}

async function start(path: string) {
  if (running) return;
  if (!looksLikeAudio(path)) {
    showError("Esse ficheiro não parece ser áudio. Escolha um ficheiro de áudio.");
    return;
  }
  running = `job-${Date.now()}`;
  drop.classList.add("busy");
  result.hidden = true;
  status.hidden = false;
  bar.value = 0;
  phase.textContent = "A começar…";
  // Cancel does nothing during the model download or the audio decode (see
  // commands.rs) — those phases end before any "transcribing" progress message
  // arrives. Keep the button disabled until then rather than offering an
  // action that silently has no effect.
  cancelBtn.disabled = true;

  try {
    const outcome = await transcribe(path, lang.value, running, (p) => {
      const { text, value } = describe(p);
      phase.textContent = text;
      bar.value = value;
      if (p.phase === "transcribing") {
        cancelBtn.disabled = false;
      }
    });
    showDone(outcome);
  } catch (e) {
    if (isCommandError(e) && e.kind === "cancelled") {
      showCancelled();
    } else {
      showError(errorMessage(e));
    }
  } finally {
    running = null;
    drop.classList.remove("busy");
    cancelBtn.disabled = true;
  }
}

pick.addEventListener("click", async () => {
  const path = await pickFile();
  if (path) await start(path);
});

cancelBtn.addEventListener("click", async () => {
  if (running) await cancel(running);
});

await onFilesDropped((paths) => {
  if (paths[0]) void start(paths[0]);
});
