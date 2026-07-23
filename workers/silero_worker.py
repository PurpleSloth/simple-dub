"""Пакетный worker Silero 5.5 для автономного одноголосого дубляжа."""

from __future__ import annotations

import argparse
import json
import wave
from pathlib import Path
from typing import Any

import torch


SPEAKERS = ("aidar", "baya", "eugene", "kseniya", "xenia")
SAMPLE_RATES = (8_000, 24_000, 48_000)


def parse_arguments() -> argparse.Namespace:
    """Разобрать стабильный CLI-контракт между Rust и worker."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--speaker", choices=SPEAKERS, required=True)
    parser.add_argument(
        "--sample-rate",
        type=int,
        choices=SAMPLE_RATES,
        default=48_000,
    )
    return parser.parse_args()


def load_model(model_path: Path) -> Any:
    """Загрузить локальную torch.package-модель без сетевых обращений."""
    if not model_path.is_file():
        raise FileNotFoundError(f"Модель Silero не найдена: {model_path}")
    model = torch.package.PackageImporter(str(model_path)).load_pickle(
        "tts_models",
        "model",
    )
    model.to(torch.device("cpu"))
    return model


def duration_ms(path: Path) -> int:
    """Получить фактическую длительность PCM WAV."""
    with wave.open(str(path), "rb") as audio:
        return round(audio.getnframes() / audio.getframerate() * 1_000)


def synthesize(
    model: Any,
    segments: list[dict[str, Any]],
    output_dir: Path,
    speaker: str,
    sample_rate: int,
) -> list[dict[str, Any]]:
    """Озвучить manifest и вернуть машинно-читаемый результат."""
    output_dir.mkdir(parents=True, exist_ok=True)
    results: list[dict[str, Any]] = []

    for position, segment in enumerate(segments):
        segment_id = str(segment.get("id", position))
        text = str(segment.get("text", "")).strip()
        output_path = output_dir / f"segment_{position:05d}.wav"
        if not text:
            results.append(
                {
                    "id": segment_id,
                    "audio_path": None,
                    "duration_ms": 0,
                    "error": "пустой текст",
                }
            )
            continue
        try:
            model.save_wav(
                text=text,
                speaker=speaker,
                sample_rate=sample_rate,
                audio_path=str(output_path),
            )
            results.append(
                {
                    "id": segment_id,
                    "audio_path": str(output_path.resolve()),
                    "duration_ms": duration_ms(output_path),
                    "error": None,
                }
            )
        except Exception as error:  # noqa: BLE001 — ошибка возвращается Rust
            results.append(
                {
                    "id": segment_id,
                    "audio_path": None,
                    "duration_ms": 0,
                    "error": str(error),
                }
            )
    return results


def main() -> int:
    """Выполнить пакетный синтез."""
    arguments = parse_arguments()
    segments = json.loads(arguments.input.read_text(encoding="utf-8"))
    if not isinstance(segments, list):
        raise ValueError("Входной manifest должен быть JSON-массивом")
    results = synthesize(
        model=load_model(arguments.model),
        segments=segments,
        output_dir=arguments.output_dir,
        speaker=arguments.speaker,
        sample_rate=arguments.sample_rate,
    )
    print(json.dumps(results, ensure_ascii=False))
    return 0 if all(item["error"] is None for item in results) else 2


if __name__ == "__main__":
    raise SystemExit(main())
