import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import "./style.css";

type StreamType = "video" | "audio" | "subtitle" | "other";
type SubtitleKind = "text" | "image" | null;
type TtsEngineId = "piper-dmitri-fp32" | "silero-v5-5-eugene";

interface StreamInfo {
  index: number;
  stream_type: StreamType;
  codec_name: string | null;
  language: string | null;
  title: string | null;
  channels: number | null;
  channel_layout: string | null;
  is_default: boolean;
  subtitle_kind: SubtitleKind;
}

interface MediaInfo {
  streams: StreamInfo[];
  chapter_count: number;
  duration_seconds: number | null;
  format_name: string | null;
}

interface OpenRouterKeyStatus {
  configured: boolean;
}

interface TtsEngineStatus {
  id: TtsEngineId;
  display_name: string;
  backend: "sherpa-onnx" | "silero-python";
  model_id: string;
  speaker: string;
  sample_rate: number;
  installed: boolean;
  status_message: string;
}

interface JobProgress {
  percent: number;
  stage: string;
  message: string;
}

interface DubJobResult {
  outputPath: string;
  segmentCount: number;
}

const TTS_ENGINE_STORAGE_KEY = "simple-dub.tts-engine";

const state: {
  path: string | null;
  media: MediaInfo | null;
  audioIndex: number | null;
  subtitleIndex: number | null;
  ttsEngine: TtsEngineId;
  ttsStatuses: TtsEngineStatus[];
} = {
  path: null,
  media: null,
  audioIndex: null,
  subtitleIndex: null,
  ttsEngine: loadStoredTtsEngine(),
  ttsStatuses: [],
};

const app = document.querySelector<HTMLElement>("#app");

if (!app) {
  throw new Error("Корневой элемент приложения не найден");
}

app.innerHTML = `
  <section class="shell">
    <header class="hero">
      <div class="brand-mark">SD</div>
      <div class="hero-copy">
        <p class="eyebrow">Одноголосый автодубляж</p>
        <h1>Simple Dub</h1>
        <p class="lede">
          Выберите видео, оригинальную дорожку и субтитры. Исходные потоки
          сохранятся, а русский дубляж будет добавлен в новый MKV.
        </p>
      </div>
      <button id="settings-toggle" class="ghost" type="button">
        Настройки
      </button>
    </header>

    <section id="settings-panel" class="card settings-card is-hidden">
      <div>
        <p class="eyebrow">OpenRouter</p>
        <h2>Ключ для перевода</h2>
        <p>
          Ключ хранится в Windows Credential Manager и не записывается
          в файлы приложения.
        </p>
      </div>
      <form id="openrouter-settings-form" class="settings-controls">
        <label for="openrouter-key">API key</label>
        <div class="secret-row">
          <input
            id="openrouter-key"
            type="password"
            autocomplete="off"
            spellcheck="false"
            placeholder="sk-or-…"
          />
          <button id="save-openrouter-key" class="primary" type="submit">
            Сохранить
          </button>
          <button id="delete-openrouter-key" class="ghost danger" type="button">
            Удалить
          </button>
        </div>
        <p id="openrouter-key-status" class="settings-status" role="status">
          Проверяем настройку…
        </p>
      </form>
    </section>

    <section class="card file-card">
      <div>
        <span class="step">1</span>
        <div>
          <h2>Исходный файл</h2>
          <p id="file-label">MKV, MP4, AVI или MOV</p>
        </div>
      </div>
      <button id="choose-file" class="primary">Выбрать видео</button>
    </section>

    <section id="track-panel" class="track-grid is-hidden">
      <article class="card">
        <div class="section-title">
          <span class="step">2</span>
          <div>
            <h2>Оригинальная дорожка</h2>
            <p>Она останется в MKV и будет основой нового микса.</p>
          </div>
        </div>
        <div id="audio-list" class="choices"></div>
      </article>

      <article class="card">
        <div class="section-title">
          <span class="step">3</span>
          <div>
            <h2>Субтитры</h2>
            <p>Русские озвучим сразу, остальные переведём.</p>
          </div>
        </div>
        <div id="subtitle-list" class="choices"></div>
      </article>
    </section>

    <section id="tts-panel" class="card tts-card is-hidden">
      <div class="section-title">
        <span class="step">4</span>
        <div>
          <h2>Голос дубляжа</h2>
          <p>
            Piper быстрее и легче устанавливается, Silero отличается более
            мягким тембром.
          </p>
        </div>
      </div>
      <div id="tts-list" class="choices tts-choices"></div>
    </section>

    <section id="summary" class="card summary is-hidden">
      <div class="summary-copy">
        <span class="status-dot"></span>
        <div>
          <p class="eyebrow">Маршрут обработки</p>
          <h2 id="route-title">Готово к подготовке</h2>
          <p id="route-description"></p>
        </div>
      </div>
      <div class="summary-actions">
        <div id="job-progress" class="job-progress is-hidden">
          <div class="progress-track">
            <span id="progress-fill"></span>
          </div>
          <small id="progress-message">Подготовка…</small>
        </div>
        <button id="prepare-job" class="primary">Начать дубляж</button>
      </div>
    </section>

    <p id="error" class="error" role="alert"></p>
  </section>
`;

const chooseButton = document.querySelector<HTMLButtonElement>("#choose-file");
const settingsToggle =
  document.querySelector<HTMLButtonElement>("#settings-toggle");
const settingsPanel = document.querySelector<HTMLElement>("#settings-panel");
const openRouterSettingsForm =
  document.querySelector<HTMLFormElement>("#openrouter-settings-form");
const openRouterKeyInput =
  document.querySelector<HTMLInputElement>("#openrouter-key");
const saveOpenRouterKeyButton =
  document.querySelector<HTMLButtonElement>("#save-openrouter-key");
const deleteOpenRouterKeyButton =
  document.querySelector<HTMLButtonElement>("#delete-openrouter-key");
const openRouterKeyStatus =
  document.querySelector<HTMLElement>("#openrouter-key-status");
const fileLabel = document.querySelector<HTMLElement>("#file-label");
const trackPanel = document.querySelector<HTMLElement>("#track-panel");
const audioList = document.querySelector<HTMLElement>("#audio-list");
const subtitleList = document.querySelector<HTMLElement>("#subtitle-list");
const ttsPanel = document.querySelector<HTMLElement>("#tts-panel");
const ttsList = document.querySelector<HTMLElement>("#tts-list");
const summary = document.querySelector<HTMLElement>("#summary");
const routeTitle = document.querySelector<HTMLElement>("#route-title");
const routeDescription =
  document.querySelector<HTMLElement>("#route-description");
const prepareButton =
  document.querySelector<HTMLButtonElement>("#prepare-job");
const jobProgress = document.querySelector<HTMLElement>("#job-progress");
const progressFill = document.querySelector<HTMLElement>("#progress-fill");
const progressMessage =
  document.querySelector<HTMLElement>("#progress-message");
const errorBox = document.querySelector<HTMLElement>("#error");

chooseButton?.addEventListener("click", chooseMedia);
settingsToggle?.addEventListener("click", toggleSettings);
openRouterSettingsForm?.addEventListener("submit", (event) => {
  event.preventDefault();
  void saveOpenRouterKey();
});
deleteOpenRouterKeyButton?.addEventListener("click", deleteOpenRouterKey);
prepareButton?.addEventListener("click", () => void startDubJob());

void refreshOpenRouterKeyStatus();
void refreshTtsEngineStatuses();
void subscribeToJobProgress();

function toggleSettings(): void {
  settingsPanel?.classList.toggle("is-hidden");
  if (!settingsPanel?.classList.contains("is-hidden")) {
    openRouterKeyInput?.focus();
  }
}

async function refreshOpenRouterKeyStatus(): Promise<void> {
  if (!isTauriRuntime()) {
    setOpenRouterStatus("Хранилище доступно в установленном приложении.", false);
    return;
  }

  try {
    const status = await invoke<OpenRouterKeyStatus>("openrouter_key_status");
    renderOpenRouterStatus(status);
  } catch (error) {
    setOpenRouterStatus(String(error), true);
  }
}

async function saveOpenRouterKey(): Promise<void> {
  const key = openRouterKeyInput?.value ?? "";
  setSettingsBusy(true);

  try {
    const status = await invoke<OpenRouterKeyStatus>("save_openrouter_key", {
      key,
    });
    if (openRouterKeyInput) {
      openRouterKeyInput.value = "";
    }
    renderOpenRouterStatus(status);
  } catch (error) {
    setOpenRouterStatus(String(error), true);
  } finally {
    setSettingsBusy(false);
  }
}

async function deleteOpenRouterKey(): Promise<void> {
  setSettingsBusy(true);

  try {
    const status = await invoke<OpenRouterKeyStatus>("delete_openrouter_key");
    if (openRouterKeyInput) {
      openRouterKeyInput.value = "";
    }
    renderOpenRouterStatus(status);
  } catch (error) {
    setOpenRouterStatus(String(error), true);
  } finally {
    setSettingsBusy(false);
  }
}

async function refreshTtsEngineStatuses(): Promise<void> {
  if (!isTauriRuntime()) {
    state.ttsStatuses = fallbackTtsStatuses();
    renderTtsChoices();
    return;
  }

  try {
    state.ttsStatuses =
      await invoke<TtsEngineStatus[]>("tts_engine_statuses");
  } catch (error) {
    showError(`Не удалось проверить TTS-компоненты: ${String(error)}`);
    state.ttsStatuses = fallbackTtsStatuses();
  }

  renderTtsChoices();
}

async function subscribeToJobProgress(): Promise<void> {
  if (!isTauriRuntime()) {
    return;
  }
  await listen<JobProgress>("job-progress", (event) => {
    renderJobProgress(event.payload);
  });
}

async function startDubJob(): Promise<void> {
  clearError();
  if (!state.path || !state.media || state.audioIndex === null) {
    showError("Сначала выберите видео и оригинальную аудиодорожку.");
    return;
  }
  if (!isTauriRuntime()) {
    showError("Запуск дубляжа доступен в desktop-приложении.");
    return;
  }

  const subtitle = state.media.streams.find(
    (stream) => stream.index === state.subtitleIndex,
  );
  prepareButton?.setAttribute("disabled", "true");
  jobProgress?.classList.remove("is-hidden");
  renderJobProgress({
    percent: 0,
    stage: "prepare",
    message: "Запуск задания…",
  });

  try {
    const result = await invoke<DubJobResult>("start_dub_job", {
      request: {
        inputPath: state.path,
        audioStreamIndex: state.audioIndex,
        subtitleStreamIndex: state.subtitleIndex,
        subtitleKind: subtitle?.subtitle_kind ?? null,
        subtitleLanguage: subtitle?.language ?? null,
        existingAudioStreams: state.media.streams.filter(
          (stream) => stream.stream_type === "audio",
        ).length,
        durationSeconds: state.media.duration_seconds ?? 0,
        ttsEngine: state.ttsEngine,
        originalVolume: 0.3,
      },
    });
    if (routeTitle && routeDescription) {
      routeTitle.textContent = "Дубляж готов";
      routeDescription.textContent =
        `${result.segmentCount} реплик · ${result.outputPath}`;
    }
  } catch (error) {
    showError(String(error));
    if (routeTitle) {
      routeTitle.textContent = "Задание остановлено";
    }
  } finally {
    prepareButton?.removeAttribute("disabled");
    await refreshTtsEngineStatuses();
  }
}

function renderJobProgress(progress: JobProgress): void {
  const percent = Math.max(0, Math.min(100, progress.percent));
  if (progressFill) {
    progressFill.style.width = `${percent}%`;
  }
  if (progressMessage) {
    progressMessage.textContent = `${percent}% · ${progress.message}`;
  }
}

function renderOpenRouterStatus(status: OpenRouterKeyStatus): void {
  setOpenRouterStatus(
    status.configured
      ? "Ключ настроен и хранится в Windows Credential Manager."
      : "Ключ пока не настроен.",
    false,
  );
  deleteOpenRouterKeyButton?.toggleAttribute("disabled", !status.configured);
}

function setOpenRouterStatus(message: string, isError: boolean): void {
  if (!openRouterKeyStatus) {
    return;
  }

  openRouterKeyStatus.textContent = message;
  openRouterKeyStatus.classList.toggle("is-error", isError);
}

function setSettingsBusy(isBusy: boolean): void {
  saveOpenRouterKeyButton?.toggleAttribute("disabled", isBusy);
  deleteOpenRouterKeyButton?.toggleAttribute("disabled", isBusy);
  openRouterKeyInput?.toggleAttribute("disabled", isBusy);
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

async function chooseMedia(): Promise<void> {
  clearError();
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [
      {
        name: "Видео",
        extensions: ["mkv", "mp4", "avi", "mov", "webm"],
      },
    ],
  });

  if (typeof selected !== "string") {
    return;
  }

  chooseButton?.setAttribute("disabled", "true");
  if (fileLabel) {
    fileLabel.textContent = "Анализ потоков…";
  }

  try {
    const media = await invoke<MediaInfo>("inspect_media", { path: selected });
    state.path = selected;
    state.media = media;
    state.audioIndex =
      media.streams.find(
        (stream) => stream.stream_type === "audio" && stream.is_default,
      )?.index ??
      media.streams.find((stream) => stream.stream_type === "audio")?.index ??
      null;
    state.subtitleIndex =
      media.streams.find(
        (stream) =>
          stream.stream_type === "subtitle" &&
          stream.subtitle_kind === "text" &&
          isRussian(stream.language),
      )?.index ?? null;
    renderMedia();
  } catch (error) {
    showError(String(error));
    if (fileLabel) {
      fileLabel.textContent = "Не удалось прочитать файл";
    }
  } finally {
    chooseButton?.removeAttribute("disabled");
  }
}

function renderMedia(): void {
  if (!state.media || !state.path) {
    return;
  }

  if (fileLabel) {
    const name = state.path.split(/[\\/]/).at(-1) ?? state.path;
    const minutes = state.media.duration_seconds
      ? Math.round(state.media.duration_seconds / 60)
      : null;
    fileLabel.textContent = `${name}${minutes ? ` · ${minutes} мин` : ""} · ${state.media.chapter_count} глав`;
  }

  renderAudioChoices();
  renderSubtitleChoices();
  renderTtsChoices();
  trackPanel?.classList.remove("is-hidden");
  ttsPanel?.classList.remove("is-hidden");
  summary?.classList.remove("is-hidden");
  updateRoute();
}

function renderAudioChoices(): void {
  if (!audioList || !state.media) {
    return;
  }

  const streams = state.media.streams.filter(
    (stream) => stream.stream_type === "audio",
  );
  audioList.innerHTML = streams
    .map(
      (stream) => `
        <label class="choice">
          <input
            type="radio"
            name="audio"
            value="${stream.index}"
            ${state.audioIndex === stream.index ? "checked" : ""}
          />
          <span>
            <strong>${escapeHtml(stream.title ?? `Дорожка ${stream.index}`)}</strong>
            <small>${trackMeta(stream)}</small>
          </span>
        </label>
      `,
    )
    .join("");

  audioList.querySelectorAll<HTMLInputElement>('input[name="audio"]').forEach(
    (input) =>
      input.addEventListener("change", () => {
        state.audioIndex = Number(input.value);
        updateRoute();
      }),
  );
}

function renderSubtitleChoices(): void {
  if (!subtitleList || !state.media) {
    return;
  }

  const streams = state.media.streams.filter(
    (stream) => stream.stream_type === "subtitle",
  );
  subtitleList.innerHTML = `
    <label class="choice">
      <input
        type="radio"
        name="subtitle"
        value="none"
        ${state.subtitleIndex === null ? "checked" : ""}
      />
      <span>
        <strong>Распознать речь</strong>
        <small>whisper.cpp · large-v3-turbo</small>
      </span>
    </label>
    ${streams
      .map(
        (stream) => `
          <label class="choice ${stream.subtitle_kind === "image" ? "muted" : ""}">
            <input
              type="radio"
              name="subtitle"
              value="${stream.index}"
              ${state.subtitleIndex === stream.index ? "checked" : ""}
            />
            <span>
              <strong>${escapeHtml(stream.title ?? `Субтитры ${stream.index}`)}</strong>
              <small>${trackMeta(stream)} · ${stream.subtitle_kind === "image" ? "графические, потребуется Whisper" : "текстовые"}</small>
            </span>
          </label>
        `,
      )
      .join("")}
  `;

  subtitleList
    .querySelectorAll<HTMLInputElement>('input[name="subtitle"]')
    .forEach((input) =>
      input.addEventListener("change", () => {
        state.subtitleIndex =
          input.value === "none" ? null : Number(input.value);
        updateRoute();
      }),
    );
}

function renderTtsChoices(): void {
  if (!ttsList) {
    return;
  }

  ttsList.innerHTML = state.ttsStatuses
    .map((status) => {
      const backend =
        status.backend === "sherpa-onnx"
          ? "Нативный ONNX"
          : "Python/PyTorch под управлением приложения";
      const readiness = status.installed
        ? "Компонент установлен"
        : "Компонент потребуется установить";
      return `
        <label class="choice tts-choice">
          <input
            type="radio"
            name="tts-engine"
            value="${status.id}"
            ${state.ttsEngine === status.id ? "checked" : ""}
          />
          <span>
            <strong>${escapeHtml(status.display_name)}</strong>
            <small>
              ${backend} · ${(status.sample_rate / 1_000).toFixed(2)} kHz
            </small>
            <small class="${status.installed ? "runtime-ready" : "runtime-missing"}">
              ${readiness}
            </small>
          </span>
        </label>
      `;
    })
    .join("");

  ttsList
    .querySelectorAll<HTMLInputElement>('input[name="tts-engine"]')
    .forEach((input) =>
      input.addEventListener("change", () => {
        state.ttsEngine = input.value as TtsEngineId;
        localStorage.setItem(TTS_ENGINE_STORAGE_KEY, state.ttsEngine);
        updateRoute();
      }),
    );
}

function updateRoute(): void {
  if (!state.media || !routeTitle || !routeDescription) {
    return;
  }

  const subtitle = state.media.streams.find(
    (stream) => stream.index === state.subtitleIndex,
  );

  if (!subtitle || subtitle.subtitle_kind === "image") {
    routeTitle.textContent = "Распознать и перевести";
    routeDescription.textContent =
      `Выбранная аудиодорожка → whisper.cpp → Gemini 3.5 Flash Lite → ${selectedTtsName()}.`;
    return;
  }

  if (isRussian(subtitle.language)) {
    routeTitle.textContent = "Озвучить русские субтитры";
    routeDescription.textContent =
      `Перевод не нужен: текст сразу отправится в ${selectedTtsName()}.`;
    return;
  }

  routeTitle.textContent = "Перевести и озвучить";
  routeDescription.textContent =
    `Текстовые субтитры → Gemini 3.5 Flash Lite → ${selectedTtsName()}.`;
}

function selectedTtsName(): string {
  return (
    state.ttsStatuses.find((status) => status.id === state.ttsEngine)
      ?.display_name ??
    (state.ttsEngine === "silero-v5-5-eugene"
      ? "Silero 5.5 · Eugene"
      : "Piper · Dmitri FP32")
  );
}

function trackMeta(stream: StreamInfo): string {
  return [
    stream.language?.toUpperCase() ?? "UND",
    stream.codec_name?.toUpperCase() ?? "UNKNOWN",
    stream.channel_layout ?? (stream.channels ? `${stream.channels} ch` : null),
    stream.is_default ? "по умолчанию" : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

function isRussian(language: string | null): boolean {
  return ["ru", "rus", "russian"].includes(language?.toLowerCase() ?? "");
}

function loadStoredTtsEngine(): TtsEngineId {
  const stored = localStorage.getItem(TTS_ENGINE_STORAGE_KEY);
  return stored === "silero-v5-5-eugene"
    ? "silero-v5-5-eugene"
    : "piper-dmitri-fp32";
}

function fallbackTtsStatuses(): TtsEngineStatus[] {
  return [
    {
      id: "piper-dmitri-fp32",
      display_name: "Piper · Dmitri FP32",
      backend: "sherpa-onnx",
      model_id: "ru_RU-dmitri-medium",
      speaker: "dmitri",
      sample_rate: 22_050,
      installed: false,
      status_message: "Статус доступен в установленном приложении.",
    },
    {
      id: "silero-v5-5-eugene",
      display_name: "Silero 5.5 · Eugene",
      backend: "silero-python",
      model_id: "v5_5_ru",
      speaker: "eugene",
      sample_rate: 48_000,
      installed: false,
      status_message: "Статус доступен в установленном приложении.",
    },
  ];
}

function escapeHtml(value: string): string {
  return value.replace(
    /[&<>"']/g,
    (character) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#039;",
      })[character] ?? character,
  );
}

function showError(message: string): void {
  if (errorBox) {
    errorBox.textContent = message;
  }
}

function clearError(): void {
  if (errorBox) {
    errorBox.textContent = "";
  }
}
