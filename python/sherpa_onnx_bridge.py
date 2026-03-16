#!/usr/bin/env python3
"""CLI bridge for local sherpa-onnx STT and TTS.

The Rust service invokes this script through the uv-managed virtualenv so it can
keep the telephony pipeline in Rust while delegating model execution to the
official Python bindings.
"""

from __future__ import annotations

import argparse
import json
import sys
import wave
from array import array
from pathlib import Path
from typing import Iterable

import sherpa_onnx


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="agent_voice sherpa-onnx bridge")
    subparsers = parser.add_subparsers(dest="command", required=True)

    stt = subparsers.add_parser("stt", help="transcribe a WAV file")
    stt.add_argument("--model-family", required=True)
    stt.add_argument("--provider", default="cpu")
    stt.add_argument("--num-threads", type=int, default=2)
    stt.add_argument("--debug", action="store_true")
    stt.add_argument("--input-wav", required=True)
    stt.add_argument("--moonshine-version", choices=("v1", "v2"), default="v2")
    stt.add_argument("--moonshine-preprocessor", default="")
    stt.add_argument("--moonshine-encoder", default="")
    stt.add_argument("--moonshine-uncached-decoder", default="")
    stt.add_argument("--moonshine-cached-decoder", default="")
    stt.add_argument("--moonshine-decoder", default="")
    stt.add_argument("--moonshine-tokens", default="")

    tts = subparsers.add_parser("tts", help="synthesize text to WAV")
    tts.add_argument("--model-family", required=True)
    tts.add_argument("--provider", default="cpu")
    tts.add_argument("--num-threads", type=int, default=2)
    tts.add_argument("--debug", action="store_true")
    tts.add_argument("--text", required=True)
    tts.add_argument("--speed", type=float, default=1.0)
    tts.add_argument("--speaker-id", type=int, default=0)
    tts.add_argument("--output-wav", required=True)
    tts.add_argument("--kokoro-model", default="")
    tts.add_argument("--kokoro-voices", default="")
    tts.add_argument("--kokoro-tokens", default="")
    tts.add_argument("--kokoro-data-dir", default="")
    tts.add_argument("--kokoro-lexicon", default="")
    tts.add_argument("--kokoro-dict-dir", default="")
    tts.add_argument("--kokoro-lang", default="")

    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    try:
        if args.command == "stt":
            payload = run_stt(args)
        else:
            payload = run_tts(args)
    except Exception as exc:  # pragma: no cover - surfaced to Rust caller
        if getattr(args, "debug", False):
            print(f"sherpa-onnx bridge failed: {exc}", file=sys.stderr)
        raise

    print(json.dumps(payload, ensure_ascii=False))
    return 0


def run_stt(args: argparse.Namespace) -> dict[str, object]:
    recognizer = build_recognizer(args)
    sample_rate, samples = read_wav(Path(args.input_wav))
    stream = recognizer.create_stream()
    stream.accept_waveform(sample_rate, normalize_samples(samples))
    recognizer.decode_stream(stream)
    result = stream.result
    return {
        "text": result.text.strip(),
        "language": getattr(result, "lang", None) or None,
    }


def run_tts(args: argparse.Namespace) -> dict[str, object]:
    tts = build_tts(args)
    generated = tts.generate(args.text, sid=args.speaker_id, speed=args.speed)
    output_path = Path(args.output_wav)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    write_wav(output_path, int(generated.sample_rate), generated.samples)
    return {
        "sample_rate": int(generated.sample_rate),
        "sample_count": len(generated.samples),
    }


def build_recognizer(args: argparse.Namespace) -> sherpa_onnx.OfflineRecognizer:
    family = normalize_family(args.model_family)
    if family in {"moonshine", "moonshine_v1"}:
        return sherpa_onnx.OfflineRecognizer.from_moonshine(
            preprocessor=args.moonshine_preprocessor,
            encoder=args.moonshine_encoder,
            uncached_decoder=args.moonshine_uncached_decoder,
            cached_decoder=args.moonshine_cached_decoder,
            tokens=args.moonshine_tokens,
            num_threads=args.num_threads,
            debug=args.debug,
            provider=args.provider,
        )
    if family == "moonshine_v2":
        return sherpa_onnx.OfflineRecognizer.from_moonshine_v2(
            encoder=args.moonshine_encoder,
            decoder=args.moonshine_decoder,
            tokens=args.moonshine_tokens,
            num_threads=args.num_threads,
            debug=args.debug,
            provider=args.provider,
        )
    raise ValueError(f"unsupported sherpa-onnx STT model family: {args.model_family}")


def build_tts(args: argparse.Namespace) -> sherpa_onnx.OfflineTts:
    family = normalize_family(args.model_family)
    if family == "kokoro":
        kokoro = sherpa_onnx.OfflineTtsKokoroModelConfig(
            model=args.kokoro_model,
            voices=args.kokoro_voices,
            tokens=args.kokoro_tokens,
            lexicon=args.kokoro_lexicon,
            data_dir=args.kokoro_data_dir,
            dict_dir=args.kokoro_dict_dir,
            lang=args.kokoro_lang,
        )
        config = sherpa_onnx.OfflineTtsConfig(
            model=sherpa_onnx.OfflineTtsModelConfig(
                kokoro=kokoro,
                num_threads=args.num_threads,
                debug=args.debug,
                provider=args.provider,
            )
        )
        return sherpa_onnx.OfflineTts(config)
    raise ValueError(f"unsupported sherpa-onnx TTS model family: {args.model_family}")


def read_wav(path: Path) -> tuple[int, list[int]]:
    with wave.open(str(path), "rb") as reader:
        sample_rate = reader.getframerate()
        channels = reader.getnchannels()
        sample_width = reader.getsampwidth()
        if sample_width != 2:
            raise ValueError(f"unsupported WAV width {sample_width * 8} bits")
        frames = reader.readframes(reader.getnframes())

    samples = array("h")
    samples.frombytes(frames)
    if sys.byteorder != "little":
        samples.byteswap()
    if channels == 1:
        return sample_rate, list(samples)
    if channels == 2:
        mono = [
            int((samples[index] + samples[index + 1]) / 2)
            for index in range(0, len(samples), 2)
        ]
        return sample_rate, mono
    raise ValueError(f"unsupported channel count {channels}")


def write_wav(path: Path, sample_rate: int, samples: Iterable[float]) -> None:
    pcm = array("h", [float_to_pcm16(sample) for sample in samples])
    if sys.byteorder != "little":
        pcm.byteswap()
    with wave.open(str(path), "wb") as writer:
        writer.setnchannels(1)
        writer.setsampwidth(2)
        writer.setframerate(sample_rate)
        writer.writeframes(pcm.tobytes())


def normalize_samples(samples: Iterable[int]) -> list[float]:
    return [max(-1.0, min(1.0, sample / 32768.0)) for sample in samples]


def float_to_pcm16(sample: float) -> int:
    clamped = max(-1.0, min(1.0, float(sample)))
    if clamped >= 1.0:
        return 32767
    if clamped <= -1.0:
        return -32768
    return int(clamped * 32767.0)


def normalize_family(value: str) -> str:
    return value.strip().replace("-", "_").lower()


if __name__ == "__main__":
    raise SystemExit(main())
