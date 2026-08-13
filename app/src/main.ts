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
const controls = el<HTMLElement>("#controls");
const pick = el<HTMLButtonElement>("#pick");
const lang = el<HTMLSelectElement>("#lang");
const status = el<HTMLElement>("#status");
const jobName = el<HTMLParagraphElement>("#job-name");
const jobFacts = el<HTMLParagraphElement>("#job-facts");
const prep = el<HTMLDivElement>("#prep");
const prepFill = el<HTMLDivElement>("#prep-fill");
const spine = el<HTMLOListElement>("#spine");
const phase = el<HTMLParagraphElement>("#phase");
const eta = el<HTMLParagraphElement>("#eta");
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

/** Matches `reflow`'s section length, so the rows shown while waiting are the
 * headings the finished document actually gets. Changing one without the other
 * would make the wait describe a document that isn't produced. */
const SECTION_SECS = 600;

function clockLabel(seconds: number): string {
  const total = Math.round(seconds);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return h > 0
    ? `${h}:${mm}:${String(s).padStart(2, "0")}`
    : `${mm}:${String(s).padStart(2, "0")}`;
}

type Section = { start: number; end: number; row: HTMLLIElement; fill: HTMLDivElement };

let sections: Section[] = [];
let duration = 0;
let transcribeStartedAt = 0;

/** Lays out one row per document section, from the audio's real length. */
function buildSpine(durationSecs: number) {
  duration = durationSecs;
  sections = [];
  spine.replaceChildren();

  const count = Math.max(1, Math.ceil(durationSecs / SECTION_SECS));
  for (let i = 0; i < count; i += 1) {
    const start = i * SECTION_SECS;
    const end = Math.min((i + 1) * SECTION_SECS, durationSecs);

    const row = document.createElement("li");
    row.dataset.state = "pending";

    const fill = document.createElement("div");
    fill.className = "fill";

    const range = document.createElement("span");
    range.className = "range";
    range.textContent = `${clockLabel(start)} – ${clockLabel(end)}`;

    row.append(fill, range);
    spine.append(row);
    sections.push({ start, end, row, fill });
  }

  prep.hidden = true;
  spine.hidden = false;
}

/** whisper works through the recording in order, so overall percentage maps
 * directly onto how far into the audio it has reached. */
function advanceSpine(pct: number) {
  const reached = (pct / 100) * duration;
  for (const s of sections) {
    const span = Math.max(1, s.end - s.start);
    const ratio = Math.min(1, Math.max(0, (reached - s.start) / span));
    s.fill.style.width = `${ratio * 100}%`;
    s.row.dataset.state = ratio >= 1 ? "done" : ratio > 0 ? "running" : "pending";
  }
}

/** Coarse on purpose: a precise countdown that is wrong reads worse than a
 * rough one that is right, and it only appears once there is enough elapsed
 * time for the estimate to mean anything. */
function updateEta(pct: number) {
  if (pct < 5 || transcribeStartedAt === 0) {
    eta.textContent = "";
    return;
  }
  const elapsed = (Date.now() - transcribeStartedAt) / 1000;
  const remaining = (elapsed * (100 - pct)) / pct;
  if (remaining < 45) {
    eta.textContent = "menos de um minuto restante";
    return;
  }
  const mins = Math.round(remaining / 60);
  eta.textContent = `cerca de ${mins} ${mins === 1 ? "minuto" : "minutos"} restantes`;
}

function apply(p: Progress) {
  switch (p.phase) {
    case "preparing":
      phase.textContent = "A preparar o modelo de voz, só desta vez";
      prep.hidden = false;
      prepFill.style.width = `${p.pct}%`;
      eta.textContent = `${p.pct}% de 574 MB`;
      break;
    case "decoding":
      phase.textContent = "A ler a gravação";
      eta.textContent = "";
      break;
    case "decoded":
      buildSpine(p.durationSecs);
      jobFacts.textContent = factsLine(p.durationSecs);
      break;
    case "transcribing":
      if (transcribeStartedAt === 0) transcribeStartedAt = Date.now();
      phase.textContent = `A transcrever · ${p.pct}%`;
      advanceSpine(p.pct);
      updateEta(p.pct);
      break;
    case "rendering":
      phase.textContent = "A escrever os documentos";
      eta.textContent = "";
      for (const s of sections) {
        s.fill.style.width = "100%";
        s.row.dataset.state = "done";
      }
      break;
  }
}

function factsLine(durationSecs: number): string {
  const mins = Math.max(1, Math.round(durationSecs / 60));
  const label = LANGUAGES.find(([code]) => code === lang.value)?.[1] ?? lang.value;
  return `${mins} min · ${label}`;
}

function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

function showError(message: string) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result error";
  result.textContent = `Erro: ${message}`;
}

// Cancellation is not an error: no alert styling, no "Erro:" prefix, a plain
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
 * that would inject and execute arbitrary HTML (the CSP in tauri.conf.json
 * blocks scripts and external resources, not inline markup assigned via the
 * DOM). Build elements and assign text via `textContent`/`append`.
 */
function showDone(o: Outcome) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result ok";
  result.replaceChildren();

  const mins = Math.max(1, Math.round(o.durationSecs / 60));
  const summary = document.createElement("p");
  const strong = document.createElement("strong");
  strong.textContent = "Pronto.";
  summary.append(strong, ` ${mins} minutos de gravação, ${o.paragraphs} parágrafos.`);

  const backend = document.createElement("p");
  backend.textContent = `Transcrito com ${o.backend}. Texto não revisto — confirme nomes e valores.`;

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

let running: string | null = null;

/** While a job runs, dropping a file and changing the language do nothing, so
 * the controls come off screen rather than sitting dimmed for twenty minutes. */
function setIdleControlsVisible(visible: boolean) {
  drop.hidden = !visible;
  controls.hidden = !visible;
}

function resetJobView(path: string) {
  jobName.textContent = basename(path);
  jobFacts.textContent = LANGUAGES.find(([c]) => c === lang.value)?.[1] ?? lang.value;
  spine.replaceChildren();
  spine.hidden = true;
  prep.hidden = true;
  prepFill.style.width = "0%";
  sections = [];
  duration = 0;
  transcribeStartedAt = 0;
  phase.textContent = "A começar";
  eta.textContent = "";
}

async function start(path: string) {
  if (running) return;
  if (!looksLikeAudio(path)) {
    showError("Esse ficheiro não parece ser uma gravação. Escolha um ficheiro de áudio.");
    return;
  }
  running = `job-${Date.now()}`;
  setIdleControlsVisible(false);
  result.hidden = true;
  status.hidden = false;
  resetJobView(path);
  // Cancel does nothing during the model download or the audio decode (see
  // commands.rs) — those phases end before any "transcribing" progress message
  // arrives. Keep the button disabled until then rather than offering an action
  // that silently has no effect.
  cancelBtn.disabled = true;

  try {
    const outcome = await transcribe(path, lang.value, running, (p) => {
      apply(p);
      if (p.phase === "transcribing") cancelBtn.disabled = false;
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
    setIdleControlsVisible(true);
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
