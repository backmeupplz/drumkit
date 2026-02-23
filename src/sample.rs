use anyhow::{Context, Result};
use std::path::Path;

/// Decoded audio sample data
pub struct SampleData {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Load a WAV file and decode it to f32 PCM samples.
/// Handles both i16 and f32 source formats.
pub fn load_wav(path: &Path) -> Result<SampleData> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("Failed to open WAV file: {}", path.display()))?;

    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels;

    let samples = match spec.sample_format {
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            let max_val = (1u32 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max_val))
                .collect::<hound::Result<Vec<f32>>>()
                .context("Failed to decode integer WAV samples")?
        }
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .collect::<hound::Result<Vec<f32>>>()
            .context("Failed to decode float WAV samples")?,
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
    fn load_wav_i16_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_i16.wav");
        // i16 max is 32767
        write_test_wav_i16(&path, 44100, 1, &[0, 16384, -16384, 32767]);

        let data = load_wav(&path).unwrap();
        assert_eq!(data.sample_rate, 44100);
        assert_eq!(data.channels, 1);
        assert_eq!(data.samples.len(), 4);
        assert!((data.samples[0] - 0.0).abs() < 0.001);
        assert!((data.samples[1] - 0.5).abs() < 0.001);
        assert!((data.samples[2] - (-0.5)).abs() < 0.001);
        // 32767 / 32768 ≈ 0.99997
        assert!((data.samples[3] - 1.0).abs() < 0.001);
    }

    #[test]
    fn load_wav_f32_stereo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_f32.wav");
        // Stereo: L R L R
        write_test_wav_f32(&path, 48000, 2, &[0.0, 0.5, -0.5, 1.0]);

        let data = load_wav(&path).unwrap();
        assert_eq!(data.sample_rate, 48000);
        assert_eq!(data.channels, 2);
        assert_eq!(data.samples.len(), 4);
        assert!((data.samples[0] - 0.0).abs() < f32::EPSILON);
        assert!((data.samples[1] - 0.5).abs() < f32::EPSILON);
        assert!((data.samples[2] - (-0.5)).abs() < f32::EPSILON);
        assert!((data.samples[3] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn load_wav_nonexistent_file() {
        let result = load_wav(Path::new("/tmp/nonexistent_drumkit_test.wav"));
        assert!(result.is_err());
    }
}
