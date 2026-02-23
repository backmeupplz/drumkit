use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::sample;

/// Metadata extracted from a sample filename.
#[derive(Debug, PartialEq)]
pub struct SampleFileInfo {
    pub note: u8,
    pub velocity_layer: Option<u8>,
    pub round_robin: Option<u8>,
}

/// Parse a WAV sample filename into note number and optional v/rr metadata.
///
/// Supported formats:
///   `36.wav`           → note=36
///   `38_v1_rr1.wav`    → note=38, v=1, rr=1
///   `38_v2_rr3.wav`    → note=38, v=2, rr=3
pub fn parse_sample_filename(filename: &str) -> Option<SampleFileInfo> {
    let stem = filename.strip_suffix(".wav")?;

    let parts: Vec<&str> = stem.split('_').collect();
    if parts.is_empty() {
        return None;
    }

    let note: u8 = parts[0].parse().ok()?;

    let mut velocity_layer = None;
    let mut round_robin = None;

    for part in &parts[1..] {
        if let Some(v) = part.strip_prefix('v') {
            velocity_layer = Some(v.parse().ok()?);
        } else if let Some(rr) = part.strip_prefix("rr") {
            round_robin = Some(rr.parse().ok()?);
        }
    }

    Some(SampleFileInfo {
        note,
        velocity_layer,
        round_robin,
    })
}

/// A loaded drum kit — maps MIDI note numbers to sample data.
#[derive(Debug)]
#[allow(dead_code)]
pub struct Kit {
    pub name: String,
    pub note_samples: HashMap<u8, Arc<Vec<f32>>>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Load all WAV files from a directory and map them by MIDI note number.
///
/// All WAVs must share the same sample rate and channel count.
/// If multiple files map to the same note, the first one alphabetically wins.
pub fn load_kit(path: &Path) -> Result<Kit> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());

    let mut entries: Vec<_> = std::fs::read_dir(path)
        .with_context(|| format!("Failed to read kit directory: {}", path.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect();

    if entries.is_empty() {
        anyhow::bail!("No WAV files found in {}", path.display());
    }

    // Sort alphabetically so the "first found" for duplicate notes is deterministic
    entries.sort_by_key(|e| e.file_name());

    let mut note_samples: HashMap<u8, Arc<Vec<f32>>> = HashMap::new();
    let mut kit_sample_rate: Option<u32> = None;
    let mut kit_channels: Option<u16> = None;

    for entry in &entries {
        let file_path = entry.path();
        let filename = entry.file_name();
        let filename_str = filename.to_string_lossy();

        let info = match parse_sample_filename(&filename_str) {
            Some(info) => info,
            None => {
                eprintln!("  Skipping {} (cannot parse note number)", filename_str);
                continue;
            }
        };

        let data = sample::load_wav(&file_path)
            .with_context(|| format!("Failed to load {}", file_path.display()))?;

        // Validate sample rate and channel count consistency
        match kit_sample_rate {
            None => kit_sample_rate = Some(data.sample_rate),
            Some(sr) if sr != data.sample_rate => {
                anyhow::bail!(
                    "Sample rate mismatch in {}: expected {} Hz, got {} Hz",
                    filename_str,
                    sr,
                    data.sample_rate
                );
            }
            _ => {}
        }

        match kit_channels {
            None => kit_channels = Some(data.channels),
            Some(ch) if ch != data.channels => {
                anyhow::bail!(
                    "Channel count mismatch in {}: expected {}, got {}",
                    filename_str,
                    ch,
                    data.channels
                );
            }
            _ => {}
        }

        // First file for this note wins (alphabetical order)
        note_samples
            .entry(info.note)
            .or_insert_with(|| Arc::new(data.samples));
    }

    if note_samples.is_empty() {
        anyhow::bail!(
            "No parseable WAV filenames in {} (expected e.g. 36.wav, 38_v1_rr1.wav)",
            path.display()
        );
    }

    let sample_rate = kit_sample_rate.unwrap();
    let channels = kit_channels.unwrap();

    // Print summary
    let mut notes: Vec<u8> = note_samples.keys().copied().collect();
    notes.sort();
    println!("Kit \"{}\" loaded: {} notes, {} Hz, {} ch", name, notes.len(), sample_rate, channels);
    for &n in &notes {
        println!("  note {:>3} → {}", n, crate::midi::drum_name(n));
    }

    Ok(Kit {
        name,
        note_samples,
        sample_rate,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_test_wav(path: &Path, sample_rate: u32, channels: u16) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..100 {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn parse_simple_note() {
        let info = parse_sample_filename("36.wav").unwrap();
        assert_eq!(info.note, 36);
        assert_eq!(info.velocity_layer, None);
        assert_eq!(info.round_robin, None);
    }

    #[test]
    fn parse_with_velocity_and_round_robin() {
        let info = parse_sample_filename("38_v1_rr1.wav").unwrap();
        assert_eq!(info.note, 38);
        assert_eq!(info.velocity_layer, Some(1));
        assert_eq!(info.round_robin, Some(1));
    }

    #[test]
    fn parse_higher_velocity_and_round_robin() {
        let info = parse_sample_filename("38_v2_rr3.wav").unwrap();
        assert_eq!(info.note, 38);
        assert_eq!(info.velocity_layer, Some(2));
        assert_eq!(info.round_robin, Some(3));
    }

    #[test]
    fn parse_not_a_number() {
        assert!(parse_sample_filename("not_a_number.wav").is_none());
    }

    #[test]
    fn parse_wrong_extension() {
        assert!(parse_sample_filename("readme.txt").is_none());
    }

    #[test]
    fn load_kit_success() {
        let dir = tempfile::tempdir().unwrap();
        write_test_wav(&dir.path().join("36.wav"), 44100, 2);
        write_test_wav(&dir.path().join("38.wav"), 44100, 2);
        write_test_wav(&dir.path().join("42.wav"), 44100, 2);

        let kit = load_kit(dir.path()).unwrap();
        assert_eq!(kit.note_samples.len(), 3);
        assert!(kit.note_samples.contains_key(&36));
        assert!(kit.note_samples.contains_key(&38));
        assert!(kit.note_samples.contains_key(&42));
        assert_eq!(kit.sample_rate, 44100);
        assert_eq!(kit.channels, 2);
    }

    #[test]
    fn load_kit_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_kit(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_kit_mismatched_sample_rates() {
        let dir = tempfile::tempdir().unwrap();
        write_test_wav(&dir.path().join("36.wav"), 44100, 2);
        write_test_wav(&dir.path().join("38.wav"), 48000, 2);

        let result = load_kit(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Sample rate mismatch"));
    }
}
