import { cancel, looksLikeAudio, onFilesDropped, pickFile, transcribe, type Outcome, type Progress } from "./api";

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

function showDone(o: Outcome) {
  status.hidden = true;
  result.hidden = false;
  result.className = "result ok";
  const mins = Math.round(o.durationSecs / 60);
  result.innerHTML = `
    <p><strong>Concluído.</strong> ${mins} minutos de áudio, ${o.paragraphs} parágrafos.</p>
    <p>Transcrito com ${o.backend}.</p>
    <p class="paths">${o.docx}<br />${o.pdf}</p>`;
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

  try {
    const outcome = await transcribe(path, lang.value, running, (p) => {
      const { text, value } = describe(p);
      phase.textContent = text;
      bar.value = value;
    });
    showDone(outcome);
  } catch (e) {
    showError(String(e));
  } finally {
    running = null;
    drop.classList.remove("busy");
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
