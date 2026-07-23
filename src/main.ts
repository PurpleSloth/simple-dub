import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./style.css";

type StreamType = "video" | "audio" | "subtitle" | "other";
type SubtitleKind = "text" | "image" | null;

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

const state: {
  path: string | null;
  media: MediaInfo | null;
  audioIndex: number | null;
  subtitleIndex: number | null;
} = {
  path: null,
  media: null,
  audioIndex: null,
  subtitleIndex: null,
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

    <section id="summary" class="card summary is-hidden">
      <div>
        <span class="status-dot"></span>
        <div>
          <p class="eyebrow">Маршрут обработки</p>
          <h2 id="route-title">Готово к подготовке</h2>
          <p id="route-description"></p>
        </div>
      </div>
      <button id="prepare-job" class="primary">Подготовить задание</button>
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
const summary = document.querySelector<HTMLElement>("#summary");
const routeTitle = document.querySelector<HTMLElement>("#route-title");
const routeDescription =
  document.querySelector<HTMLElement>("#route-description");
const prepareButton =
  document.querySelector<HTMLButtonElement>("#prepare-job");
const errorBox = document.querySelector<HTMLElement>("#error");

chooseButton?.addEventListener("click", chooseMedia);
settingsToggle?.addEventListener("click", toggleSettings);
openRouterSettingsForm?.addEventListener("submit", (event) => {
  event.preventDefault();
  void saveOpenRouterKey();
});
deleteOpenRouterKeyButton?.addEventListener("click", deleteOpenRouterKey);
prepareButton?.addEventListener("click", () => {
  if (routeTitle && routeDescription) {
    routeTitle.textContent = "Первый этап готов";
    routeDescription.textContent =
      "Потоки выбраны. Следующий вертикальный срез запустит TTS, перевод или whisper.cpp и соберёт MKV.";
  }
});

void refreshOpenRouterKeyStatus();

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
  trackPanel?.classList.remove("is-hidden");
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
      "Выбранная аудиодорожка → whisper.cpp → Gemini 3.5 Flash Lite → Silero v5_5_ru.";
    return;
  }

  if (isRussian(subtitle.language)) {
    routeTitle.textContent = "Озвучить русские субтитры";
    routeDescription.textContent =
      "Перевод не нужен: текст сразу отправится в Silero v5_5_ru.";
    return;
  }

  routeTitle.textContent = "Перевести и озвучить";
  routeDescription.textContent =
    "Текстовые субтитры → Gemini 3.5 Flash Lite → Silero v5_5_ru.";
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
