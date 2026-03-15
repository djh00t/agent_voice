//! Audio helpers for telephony framing, WAV handling, and simple resampling.

use anyhow::{Context, Result, bail};

/// The telephony sample rate used for SIP media bridging.
pub const TELEPHONY_RATE: u32 = 8_000;
/// The number of mono samples in a 20 ms telephony frame at [`TELEPHONY_RATE`].
pub const TELEPHONY_FRAME_SAMPLES: usize = 160;

/// Encodes linear PCM samples into G.711 mu-law bytes.
pub fn encode_mulaw(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .map(|sample| linear_to_mulaw(*sample))
        .collect()
}

/// Encodes mono 16-bit PCM samples into a WAV payload.
pub fn encode_wav_mono_i16(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(&mut cursor, spec).context("invalid WAV writer")?;
    for sample in samples {
        writer
            .write_sample(*sample)
            .context("failed to write WAV sample")?;
    }
    writer.finalize().context("failed to finalize WAV")?;
    Ok(cursor.into_inner())
}

/// Decodes a mono or stereo 16-bit PCM WAV payload into mono samples.
pub fn decode_wav_mono_i16(input: &[u8]) -> Result<(u32, Vec<i16>)> {
    let cursor = std::io::Cursor::new(input);
    let mut reader = match hound::WavReader::new(cursor) {
        Ok(reader) => reader,
        Err(_) => return decode_streaming_wav_mono_i16(input),
    };
    let spec = reader.spec();
    if spec.bits_per_sample != 16 {
        bail!("unsupported WAV bit depth {}", spec.bits_per_sample);
    }
    if spec.sample_format != hound::SampleFormat::Int {
        bail!("unsupported WAV sample format {:?}", spec.sample_format);
    }

    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .context("failed to read WAV samples")?;

    let mono = if spec.channels == 1 {
        samples
    } else if spec.channels == 2 {
        stereo_to_mono(&samples)
    } else {
        bail!("unsupported channel count {}", spec.channels);
    };

    Ok((spec.sample_rate, mono))
}

fn decode_streaming_wav_mono_i16(input: &[u8]) -> Result<(u32, Vec<i16>)> {
    if input.len() < 12 || &input[0..4] != b"RIFF" || &input[8..12] != b"WAVE" {
        bail!("invalid WAV payload");
    }

    let mut cursor = 12usize;
    let mut sample_rate = None;
    let mut channels = None;
    let mut bits_per_sample = None;
    let mut data = None;

    while cursor + 8 <= input.len() {
        let chunk_id = &input[cursor..cursor + 4];
        let chunk_size = u32::from_le_bytes(
            input[cursor + 4..cursor + 8]
                .try_into()
                .expect("chunk size slice length is fixed"),
        ) as usize;
        let chunk_start = cursor + 8;
        if chunk_start > input.len() {
            break;
        }

        let remaining = input.len() - chunk_start;
        let actual_len = if chunk_size == u32::MAX as usize || chunk_size > remaining {
            remaining
        } else {
            chunk_size
        };
        let chunk = &input[chunk_start..chunk_start + actual_len];

        match chunk_id {
            b"fmt " => {
                if chunk.len() < 16 {
                    bail!("invalid WAV fmt chunk");
                }
                let audio_format = u16::from_le_bytes([chunk[0], chunk[1]]);
                if audio_format != 1 {
                    bail!("unsupported WAV format {}", audio_format);
                }
                channels = Some(u16::from_le_bytes([chunk[2], chunk[3]]));
                sample_rate = Some(u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
                bits_per_sample = Some(u16::from_le_bytes([chunk[14], chunk[15]]));
            }
            b"data" => {
                data = Some(chunk);
                if chunk_size == u32::MAX as usize {
                    break;
                }
            }
            _ => {}
        }

        let padded_len = if actual_len % 2 == 0 {
            actual_len
        } else {
            actual_len + 1
        };
        cursor = chunk_start.saturating_add(padded_len);
    }

    let sample_rate = sample_rate.ok_or_else(|| anyhow::anyhow!("missing WAV fmt chunk"))?;
    let channels = channels.ok_or_else(|| anyhow::anyhow!("missing WAV channel count"))?;
    let bits_per_sample =
        bits_per_sample.ok_or_else(|| anyhow::anyhow!("missing WAV bit depth"))?;
    if bits_per_sample != 16 {
        bail!("unsupported WAV bit depth {}", bits_per_sample);
    }

    let data = data.ok_or_else(|| anyhow::anyhow!("missing WAV data chunk"))?;
    let sample_count = data.len() / 2;
    let samples = data[..sample_count * 2]
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();

    let mono = match channels {
        1 => samples,
        2 => stereo_to_mono(&samples),
        _ => bail!("unsupported channel count {}", channels),
    };

    Ok((sample_rate, mono))
}

/// Resamples mono PCM audio using linear interpolation.
pub fn resample_linear_mono(input: &[i16], input_rate: u32, output_rate: u32) -> Vec<i16> {
    if input.is_empty() || input_rate == output_rate {
        return input.to_vec();
    }
    let ratio = output_rate as f64 / input_rate as f64;
    let output_len = ((input.len() as f64) * ratio).round().max(1.0) as usize;
    let mut output = Vec::with_capacity(output_len);

    for index in 0..output_len {
        let position = index as f64 / ratio;
        let lower = position.floor() as usize;
        let upper = lower.saturating_add(1).min(input.len().saturating_sub(1));
        let frac = position - lower as f64;
        let lower_sample = input[lower] as f64;
        let upper_sample = input[upper] as f64;
        let sample = lower_sample + (upper_sample - lower_sample) * frac;
        output.push(sample.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16);
    }

    output
}

/// Splits PCM samples into fixed-width frames, zero-padding the last frame.
pub fn split_frames(samples: &[i16], frame_samples: usize) -> Vec<Vec<i16>> {
    let mut frames = Vec::new();
    let mut index = 0;
    while index < samples.len() {
        let end = (index + frame_samples).min(samples.len());
        let mut frame = samples[index..end].to_vec();
        if frame.len() < frame_samples {
            frame.resize(frame_samples, 0);
        }
        frames.push(frame);
        index = end;
    }
    if frames.is_empty() {
        frames.push(vec![0; frame_samples]);
    }
    frames
}

fn stereo_to_mono(samples: &[i16]) -> Vec<i16> {
    samples
        .chunks_exact(2)
        .map(|pair| ((pair[0] as i32 + pair[1] as i32) / 2) as i16)
        .collect()
}

fn linear_to_mulaw(sample: i16) -> u8 {
    const BIAS: i16 = 0x84;
    const CLIP: i16 = 32_635;

    let mut pcm = sample;
    let sign = if pcm < 0 {
        pcm = pcm.saturating_neg();
        0x7f
    } else {
        0xff
    };

    let clipped = pcm.min(CLIP).saturating_add(BIAS);
    let exponent = match clipped {
        0..=0x1f => 0,
        0x20..=0x3f => 1,
        0x40..=0x7f => 2,
        0x80..=0xff => 3,
        0x100..=0x1ff => 4,
        0x200..=0x3ff => 5,
        0x400..=0x7ff => 6,
        _ => 7,
    };
    let mantissa = (clipped >> (exponent + 3)) & 0x0f;

    sign ^ ((exponent << 4) as u8 | mantissa as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frames_zero_pads_last_frame() {
        let frames = split_frames(&[1, 2, 3], 4);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], vec![1, 2, 3, 0]);
    }

    #[test]
    fn resample_linear_changes_length() {
        let input = vec![0_i16; 8_000];
        let output = resample_linear_mono(&input, 8_000, 16_000);
        assert_eq!(output.len(), 16_000);
    }

    #[test]
    fn encode_mulaw_preserves_frame_count() {
        let encoded = encode_mulaw(&[0, 1, -1, 1234, -4321]);
        assert_eq!(encoded.len(), 5);
    }

    #[test]
    fn encode_wav_round_trips_samples() {
        let wav = encode_wav_mono_i16(&[0, 1024, -1024], 8_000).expect("wav");
        let (sample_rate, samples) = decode_wav_mono_i16(&wav).expect("decode");
        assert_eq!(sample_rate, 8_000);
        assert_eq!(samples, vec![0, 1024, -1024]);
    }

    #[test]
    fn decode_streaming_wav_with_unknown_sizes() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&u32::MAX.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&24_000u32.to_le_bytes());
        wav.extend_from_slice(&(24_000u32 * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&u32::MAX.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav.extend_from_slice(&1024i16.to_le_bytes());

        let (sample_rate, samples) = decode_wav_mono_i16(&wav).expect("streaming wav decodes");
        assert_eq!(sample_rate, 24_000);
        assert_eq!(samples, vec![0, 1024]);
    }
}
