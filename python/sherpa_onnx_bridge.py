#!/usr/bin/env python3
"""CLI bridge for local sherpa-onnx STT and TTS.

The Rust service invokes this script through the uv-managed virtualenv so it can
keep the telephony pipeline in Rust while delegating model execution to the
official Python bindings.
"""

from __future__ import annotations

import argparse
import base64
import io
import json
import sys
import time
import wave
from array import array
from pathlib import Path
from typing import Iterable

import sherpa_onnx


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="agent_voice sherpa-onnx bridge")
    subparsers = parser.add_subparsers(dest="command", required=True)

    stt = subparsers.add_parser("stt", help="transcribe a WAV file")
    add_stt_arguments(stt)
    stt.add_argument("--input-wav", required=True)

    serve_stt = subparsers.add_parser("serve-stt", help="serve STT requests over stdin/stdout")
    add_stt_arguments(serve_stt)
    serve_stt.add_argument("--warmup", action="store_true")

    tts = subparsers.add_parser("tts", help="synthesize text to WAV")
    add_tts_arguments(tts)
    tts.add_argument("--text", required=True)
    tts.add_argument("--output-wav", required=True)

    serve_tts = subparsers.add_parser("serve-tts", help="serve TTS requests over stdin/stdout")
    add_tts_arguments(serve_tts)
    serve_tts.add_argument("--warmup", action="store_true")

    return parser


def add_stt_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--model-family", required=True)
    parser.add_argument("--provider", default="cpu")
    parser.add_argument("--num-threads", type=int, default=2)
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--moonshine-version", choices=("v1", "v2"), default="v2")
    parser.add_argument("--moonshine-preprocessor", default="")
    parser.add_argument("--moonshine-encoder", default="")
    parser.add_argument("--moonshine-uncached-decoder", default="")
    parser.add_argument("--moonshine-cached-decoder", default="")
    parser.add_argument("--moonshine-decoder", default="")
    parser.add_argument("--moonshine-tokens", default="")


def add_tts_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--model-family", required=True)
    parser.add_argument("--provider", default="cpu")
    parser.add_argument("--num-threads", type=int, default=2)
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--speed", type=float, default=1.0)
    parser.add_argument("--speaker-id", type=int, default=0)
    parser.add_argument("--kokoro-model", default="")
    parser.add_argument("--kokoro-voices", default="")
    parser.add_argument("--kokoro-tokens", default="")
    parser.add_argument("--kokoro-data-dir", default="")
    parser.add_argument("--kokoro-lexicon", default="")
    parser.add_argument("--kokoro-dict-dir", default="")
    parser.add_argument("--kokoro-lang", default="")


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    try:
        if args.command == "stt":
            payload = run_stt(args)
            emit_json(payload)
            return 0
        if args.command == "serve-stt":
            serve_stt(args)
            return 0
        if args.command == "tts":
            payload = run_tts(args)
            emit_json(payload)
            return 0
        serve_tts(args)
        return 0
    except Exception as exc:  # pragma: no cover - surfaced to Rust caller
        if getattr(args, "debug", False):
            print(f"sherpa-onnx bridge failed: {exc}", file=sys.stderr)
        raise


def run_stt(args: argparse.Namespace) -> dict[str, object]:
    recognizer = build_recognizer(args)
    wav_bytes = Path(args.input_wav).read_bytes()
    return transcribe_wav_bytes(recognizer, wav_bytes)


def serve_stt(args: argparse.Namespace) -> None:
    started_at = time.perf_counter()
    recognizer = build_recognizer(args)
    load_ms = elapsed_ms(started_at)
    warmup_ms = 0
    if args.warmup:
        warmup_started_at = time.perf_counter()
        warmup_stt(recognizer)
        warmup_ms = elapsed_ms(warmup_started_at)
    emit_json(
        {
            "ok": True,
            "model": describe_stt_model(args),
            "load_ms": load_ms,
            "warmup_ms": warmup_ms,
        }
    )

    for raw_line in sys.stdin:
        line = raw_line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            wav_bytes = base64.b64decode(request["wav_b64"])
            emit_json(ok_result(transcribe_wav_bytes(recognizer, wav_bytes)))
        except Exception as exc:  # pragma: no cover - runtime protocol path
            emit_json(error_result(str(exc)))


def run_tts(args: argparse.Namespace) -> dict[str, object]:
    tts = build_tts(args)
    generated = generate_tts(tts, args, args.text, args.speaker_id)
    output_path = Path(args.output_wav)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    write_wav(output_path, int(generated.sample_rate), generated.samples)
    return {
        "sample_rate": int(generated.sample_rate),
        "sample_count": len(generated.samples),
    }


def serve_tts(args: argparse.Namespace) -> None:
    started_at = time.perf_counter()
    tts = build_tts(args)
    load_ms = elapsed_ms(started_at)
    warmup_ms = 0
    if args.warmup:
        warmup_started_at = time.perf_counter()
        warmup_tts(tts, args)
        warmup_ms = elapsed_ms(warmup_started_at)
    emit_json(
        {
            "ok": True,
            "model": describe_tts_model(args),
            "load_ms": load_ms,
            "warmup_ms": warmup_ms,
        }
    )

    for raw_line in sys.stdin:
        line = raw_line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            generated = generate_tts(
                tts,
                args,
                request["text"],
                int(request.get("speaker_id", args.speaker_id)),
            )
            emit_json(
                ok_result(
                    {
                        "sample_rate": int(generated.sample_rate),
                        "sample_count": len(generated.samples),
                        "pcm_s16le_b64": encode_pcm16(generated.samples),
                    }
                )
            )
        except Exception as exc:  # pragma: no cover - runtime protocol path
            emit_json(error_result(str(exc)))


def transcribe_wav_bytes(
    recognizer: sherpa_onnx.OfflineRecognizer, wav_bytes: bytes
) -> dict[str, object]:
    sample_rate, samples = read_wav_bytes(wav_bytes)
    stream = recognizer.create_stream()
    stream.accept_waveform(sample_rate, normalize_samples(samples))
    recognizer.decode_stream(stream)
    result = stream.result
    return {
        "text": result.text.strip(),
        "language": getattr(result, "lang", None) or None,
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


def generate_tts(
    tts: sherpa_onnx.OfflineTts,
    args: argparse.Namespace,
    text: str,
    speaker_id: int,
) -> object:
    return tts.generate(text, sid=speaker_id, speed=args.speed)


def warmup_stt(recognizer: sherpa_onnx.OfflineRecognizer) -> None:
    stream = recognizer.create_stream()
    stream.accept_waveform(16000, [0.0] * 1600)
    recognizer.decode_stream(stream)
    _ = stream.result


def warmup_tts(tts: sherpa_onnx.OfflineTts, args: argparse.Namespace) -> None:
    _ = tts.generate("Hi", sid=args.speaker_id, speed=args.speed)


def read_wav_bytes(payload: bytes) -> tuple[int, list[int]]:
    with wave.open(io.BytesIO(payload), "rb") as reader:
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


def encode_pcm16(samples: Iterable[float]) -> str:
    pcm = array("h", [float_to_pcm16(sample) for sample in samples])
    if sys.byteorder != "little":
        pcm.byteswap()
    return base64.b64encode(pcm.tobytes()).decode("ascii")


def float_to_pcm16(sample: float) -> int:
    clamped = max(-1.0, min(1.0, float(sample)))
    if clamped >= 1.0:
        return 32767
    if clamped <= -1.0:
        return -32768
    return int(clamped * 32767.0)


def describe_stt_model(args: argparse.Namespace) -> str:
    family = normalize_family(args.model_family)
    if family in {"moonshine", "moonshine_v1"}:
        return "sherpa-onnx-moonshine-v1"
    if family == "moonshine_v2":
        return "sherpa-onnx-moonshine-v2"
    return f"sherpa-onnx-{family}"


def describe_tts_model(args: argparse.Namespace) -> str:
    family = normalize_family(args.model_family)
    return f"sherpa-onnx-{family}"


def ok_result(result: dict[str, object]) -> dict[str, object]:
    return {"ok": True, "result": result}


def error_result(message: str) -> dict[str, object]:
    return {"ok": False, "error": message}


def emit_json(payload: dict[str, object]) -> None:
    print(json.dumps(payload, ensure_ascii=False), flush=True)


def normalize_family(value: str) -> str:
    return value.strip().replace("-", "_").lower()


def elapsed_ms(started_at: float) -> int:
    return int((time.perf_counter() - started_at) * 1000)


if __name__ == "__main__":
    raise SystemExit(main())
