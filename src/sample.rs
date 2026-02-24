use anyhow::{Context, Result};
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decoded audio sample data
pub struct SampleData {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Load an audio file and decode it to interleaved f32 PCM samples.
/// Supports all formats handled by Symphonia: WAV, MP3, FLAC, OGG Vorbis, AAC, ALAC, ADPCM.
pub fn load_audio(path: &Path) -> Result<SampleData> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open audio file: {}", path.display()))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Provide an extension hint so Symphonia can pick the right demuxer
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("Failed to probe audio format: {}", path.display()))?;

    let mut format_reader = probed.format;

    let track = format_reader
        .default_track()
        .context("No audio tracks found")?;

    let sample_rate = track
        .codec_params
        .sample_rate
        .context("Missing sample rate")?;
    let channels = track
        .codec_params
        .channels
        .context("Missing channel info")?
        .count() as u16;
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("Failed to create audio decoder")?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format_reader.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e).context("Error reading audio packet"),
        };

        // Skip packets from other tracks
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet).context("Failed to decode audio packet")?;

        let spec = *decoded.spec();
        let num_frames = decoded.frames();
        let num_channels = spec.channels.count();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        all_samples.extend_from_slice(sample_buf.samples());
        // Trim to exact interleaved sample count (frames × channels)
        let expected = num_frames * num_channels;
        let current_len = all_samples.len();
        if sample_buf.samples().len() > expected {
            all_samples.truncate(current_len - (sample_buf.samples().len() - expected));
        }
    }

    // Convert mono to stereo by duplicating each sample to both channels
    let (samples, channels) = if channels == 1 {
        let stereo: Vec<f32> = all_samples.iter().flat_map(|&s| [s, s]).collect();
        (stereo, 2)
    } else {
        (all_samples, channels)
    };

    Ok(SampleData {
        samples,
        sample_rate,
        channels,
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal WAV in memory and write it to a temp file for testing.
    fn write_test_wav_i16(path: &Path, sample_rate: u32, channels: u16, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn write_test_wav_f32(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &s in samples {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn load_audio_i16_mono_converts_to_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_i16.wav");
        // i16 max is 32767
        write_test_wav_i16(&path, 44100, 1, &[0, 16384, -16384, 32767]);

        let data = load_audio(&path).unwrap();
        assert_eq!(data.sample_rate, 44100);
        assert_eq!(data.channels, 2); // mono converted to stereo
        assert_eq!(data.samples.len(), 8); // 4 mono samples → 8 stereo samples
        // Each mono sample duplicated to L and R
        assert!((data.samples[0] - 0.0).abs() < 0.001); // L
        assert!((data.samples[1] - 0.0).abs() < 0.001); // R
        assert!((data.samples[2] - 0.5).abs() < 0.001); // L
        assert!((data.samples[3] - 0.5).abs() < 0.001); // R
        assert!((data.samples[4] - (-0.5)).abs() < 0.001);
        assert!((data.samples[5] - (-0.5)).abs() < 0.001);
        assert!((data.samples[6] - 1.0).abs() < 0.001);
        assert!((data.samples[7] - 1.0).abs() < 0.001);
    }

    #[test]
    fn load_audio_f32_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_f32.wav");
        // Stereo: L R L R
        write_test_wav_f32(&path, 48000, 2, &[0.0, 0.5, -0.5, 1.0]);

        let data = load_audio(&path).unwrap();
        assert_eq!(data.sample_rate, 48000);
        assert_eq!(data.channels, 2);
        assert_eq!(data.samples.len(), 4);
        assert!((data.samples[0] - 0.0).abs() < f32::EPSILON);
        assert!((data.samples[1] - 0.5).abs() < f32::EPSILON);
        assert!((data.samples[2] - (-0.5)).abs() < f32::EPSILON);
        assert!((data.samples[3] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn load_audio_nonexistent_file() {
        let result = load_audio(Path::new("/tmp/nonexistent_drumkit_test.wav"));
        assert!(result.is_err());
    }
}
