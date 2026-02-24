use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::sample;

/// Supported audio file extensions (Symphonia-backed).
const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg", "aac", "m4a"];

/// Check whether the given file extension is a supported audio format.
fn is_audio_file(ext: &OsStr) -> bool {
    AUDIO_EXTENSIONS
        .iter()
        .any(|&e| ext.eq_ignore_ascii_case(e))
}

/// Metadata extracted from a sample filename.
#[derive(Debug, PartialEq)]
pub struct SampleFileInfo {
    pub note: u8,
    pub velocity_layer: Option<u8>,
    pub round_robin: Option<u8>,
}

/// Parse an audio sample filename into note number and optional v/rr metadata.
///
/// Supported formats:
///   `36.wav`           → note=36
///   `38_v1_rr1.flac`   → note=38, v=1, rr=1
///   `38_v2_rr3.mp3`    → note=38, v=2, rr=3
pub fn parse_sample_filename(filename: &str) -> Option<SampleFileInfo> {
    let stem = AUDIO_EXTENSIONS
        .iter()
        .find_map(|ext| filename.strip_suffix(&format!(".{ext}")))?;

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

/// A single sample variant with its velocity layer and round-robin index.
#[derive(Debug)]
pub struct SampleVariant {
    pub samples: Arc<Vec<f32>>,
    pub velocity_layer: u8,
    pub round_robin: u8,
}

/// All variants for a single MIDI note, supporting velocity layers and round-robin.
// Manual Debug impl below (AtomicUsize doesn't derive Debug automatically in all contexts)
pub struct NoteGroup {
    pub variants: Vec<SampleVariant>,
    pub max_velocity_layer: u8,
    pub max_round_robin: u8,
    rr_counter: AtomicUsize,
}

impl std::fmt::Debug for NoteGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteGroup")
            .field("variants", &self.variants)
            .field("max_velocity_layer", &self.max_velocity_layer)
            .field("max_round_robin", &self.max_round_robin)
            .field("rr_counter", &self.rr_counter.load(Ordering::Relaxed))
            .finish()
    }
}

impl NoteGroup {
    /// Select a sample based on MIDI velocity (1-127).
    ///
    /// Maps velocity to the appropriate velocity layer, then cycles through
    /// round-robin variants to avoid the machine-gun effect.
    pub fn select(&self, velocity: u8) -> Option<&Arc<Vec<f32>>> {
        let velocity = velocity.max(1);

        // Map velocity (1-127) to layer (1..=max_v)
        // ceil(velocity * max_v / 127)
        let layer = ((velocity as u16 * self.max_velocity_layer as u16 + 126) / 127)
            .clamp(1, self.max_velocity_layer as u16) as u8;

        // Count how many RR variants exist for this layer
        let rr_count = self
            .variants
            .iter()
            .filter(|v| v.velocity_layer == layer)
            .count();

        if rr_count == 0 {
            return None;
        }

        // Advance RR counter atomically and wrap
        let rr_index = self.rr_counter.fetch_add(1, Ordering::Relaxed) % rr_count;

        // Find the nth variant matching this layer
        self.variants
            .iter()
            .filter(|v| v.velocity_layer == layer)
            .nth(rr_index)
            .map(|v| &v.samples)
    }
}

/// A loaded drum kit — maps MIDI note numbers to sample groups.
#[derive(Debug)]
#[allow(dead_code)]
pub struct Kit {
    pub name: String,
    pub notes: HashMap<u8, Arc<NoteGroup>>,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Return sorted note keys for a kit's note map.
pub fn note_keys(notes: &HashMap<u8, Arc<NoteGroup>>) -> Vec<u8> {
    let mut keys: Vec<u8> = notes.keys().copied().collect();
    keys.sort();
    keys
}

/// Return kit loading summary as log lines (for the TUI log viewer).
pub fn summary_lines(kit: &Kit, mapping: &crate::mapping::NoteMapping) -> Vec<String> {
    let keys = note_keys(&kit.notes);
    let mut lines = Vec::with_capacity(keys.len() + 1);
    lines.push(format!(
        "Kit \"{}\" loaded: {} notes, {} Hz, {} ch",
        kit.name,
        keys.len(),
        kit.sample_rate,
        kit.channels
    ));
    for &n in &keys {
        let group = &kit.notes[&n];
        let variant_info = if group.max_velocity_layer > 1 || group.max_round_robin > 1 {
            format!(
                " ({}v x {}rr = {} variants)",
                group.max_velocity_layer,
                group.max_round_robin,
                group.variants.len()
            )
        } else {
            String::new()
        };
        lines.push(format!(
            "  note {:>3} → {}{}",
            n,
            mapping.drum_name(n),
            variant_info
        ));
    }
    lines
}

/// Shared loading logic for `load_kit` and `load_kit_with_progress`.
///
/// `on_entries_counted` is called once with the total file count after reading the directory.
/// `on_file_done` is called after each file is processed (whether loaded or skipped).
fn load_kit_inner(
    path: &Path,
    on_entries_counted: &mut dyn FnMut(usize),
    on_file_done: &mut dyn FnMut(),
) -> Result<Kit> {
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
                .is_some_and(|ext| is_audio_file(ext))
        })
        .collect();

    if entries.is_empty() {
        anyhow::bail!("No audio files found in {}", path.display());
    }

    // Sort alphabetically for deterministic ordering
    entries.sort_by_key(|e| e.file_name());
    on_entries_counted(entries.len());

    let mut variants_map: HashMap<u8, Vec<SampleVariant>> = HashMap::new();
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
                on_file_done();
                continue;
            }
        };

        let data = sample::load_audio(&file_path)
            .with_context(|| format!("Failed to load {}", file_path.display()))?;

        on_file_done();

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

        let variant = SampleVariant {
            samples: Arc::new(data.samples),
            velocity_layer: info.velocity_layer.unwrap_or(1),
            round_robin: info.round_robin.unwrap_or(1),
        };

        variants_map.entry(info.note).or_default().push(variant);
    }

    if variants_map.is_empty() {
        anyhow::bail!(
            "No parseable audio filenames in {} (expected e.g. 36.wav, 38_v1_rr1.flac)",
            path.display()
        );
    }

    let sample_rate = kit_sample_rate.unwrap();
    let channels = kit_channels.unwrap();

    // Build NoteGroups from collected variants
    let mut notes: HashMap<u8, Arc<NoteGroup>> = HashMap::new();
    for (note, mut variants) in variants_map {
        // Sort by (velocity_layer, round_robin) for deterministic ordering
        variants.sort_by(|a, b| {
            a.velocity_layer
                .cmp(&b.velocity_layer)
                .then(a.round_robin.cmp(&b.round_robin))
        });

        let max_velocity_layer = variants.iter().map(|v| v.velocity_layer).max().unwrap_or(1);
        let max_round_robin = variants.iter().map(|v| v.round_robin).max().unwrap_or(1);

        notes.insert(
            note,
            Arc::new(NoteGroup {
                variants,
                max_velocity_layer,
                max_round_robin,
                rr_counter: AtomicUsize::new(0),
            }),
        );
    }

    Ok(Kit {
        name,
        notes,
        sample_rate,
        channels,
    })
}

/// Load all audio files from a directory and map them by MIDI note number.
///
/// All samples must share the same sample rate and channel count.
/// Files with the same note number are grouped by velocity layer and round-robin.
pub fn load_kit(path: &Path) -> Result<Kit> {
    load_kit_inner(path, &mut |_| {}, &mut || {})
}

/// Like `load_kit`, but reports progress via atomic counters so a UI thread
/// can display a progress bar while loading runs in the background.
pub fn load_kit_with_progress(
    path: &Path,
    progress: &Arc<AtomicUsize>,
    total: &Arc<AtomicUsize>,
) -> Result<Kit> {
    load_kit_inner(
        path,
        &mut |count| { total.store(count, Ordering::Relaxed); },
        &mut || { progress.fetch_add(1, Ordering::Relaxed); },
    )
}

/// A kit directory discovered on disk.
pub struct DiscoveredKit {
    pub name: String,
    pub path: PathBuf,
    pub wav_count: usize,
}

/// Return the built-in search directories (for display purposes).
pub fn default_search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("./kits")];
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(xdg).join("drumkit/kits"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/drumkit/kits"));
    }
    dirs
}

/// Scan standard locations for kit directories containing .wav files.
pub fn discover_kits(extra_dirs: &[PathBuf]) -> Vec<DiscoveredKit> {
    let mut kits = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    let mut search_dirs = vec![PathBuf::from("./kits")];

    // XDG_DATA_HOME or ~/.local/share
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        search_dirs.push(PathBuf::from(xdg).join("drumkit/kits"));
    } else if let Ok(home) = std::env::var("HOME") {
        search_dirs.push(PathBuf::from(home).join(".local/share/drumkit/kits"));
    }

    // Append user-supplied extra directories
    search_dirs.extend_from_slice(extra_dirs);

    for search_dir in &search_dirs {
        let entries = match std::fs::read_dir(search_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let canonical = match path.canonicalize() {
                Ok(c) => c,
                Err(_) => continue,
            };

            if !seen_paths.insert(canonical) {
                continue;
            }

            let wav_count = std::fs::read_dir(&path)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| {
                            e.path()
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
                        })
                        .count()
                })
                .unwrap_or(0);

            if wav_count == 0 {
                continue;
            }

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unnamed".to_string());

            kits.push(DiscoveredKit {
                name,
                path,
                wav_count,
            });
        }
    }

    kits.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    kits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_test_wav(path: &Path, sample_rate: u32, channels: u16) {
        write_test_wav_value(path, sample_rate, channels, 0);
    }

    fn write_test_wav_value(path: &Path, sample_rate: u32, channels: u16, value: i16) {
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..100 {
            writer.write_sample(value).unwrap();
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
    fn parse_other_audio_extensions() {
        let info = parse_sample_filename("36.flac").unwrap();
        assert_eq!(info.note, 36);

        let info = parse_sample_filename("38_v1_rr1.mp3").unwrap();
        assert_eq!(info.note, 38);
        assert_eq!(info.velocity_layer, Some(1));

        let info = parse_sample_filename("42.ogg").unwrap();
        assert_eq!(info.note, 42);

        let info = parse_sample_filename("44.m4a").unwrap();
        assert_eq!(info.note, 44);

        let info = parse_sample_filename("46.aac").unwrap();
        assert_eq!(info.note, 46);
    }

    #[test]
    fn load_kit_success() {
        let dir = tempfile::tempdir().unwrap();
        write_test_wav(&dir.path().join("36.wav"), 44100, 2);
        write_test_wav(&dir.path().join("38.wav"), 44100, 2);
        write_test_wav(&dir.path().join("42.wav"), 44100, 2);

        let kit = load_kit(dir.path()).unwrap();
        assert_eq!(kit.notes.len(), 3);
        assert!(kit.notes.contains_key(&36));
        assert!(kit.notes.contains_key(&38));
        assert!(kit.notes.contains_key(&42));
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

    // --- Stage 5: velocity layer + round-robin tests ---

    #[test]
    fn select_single_variant() {
        let group = NoteGroup {
            variants: vec![SampleVariant {
                samples: Arc::new(vec![1.0; 10]),
                velocity_layer: 1,
                round_robin: 1,
            }],
            max_velocity_layer: 1,
            max_round_robin: 1,
            rr_counter: AtomicUsize::new(0),
        };

        // Any velocity should return the same sample
        assert!(group.select(1).is_some());
        assert!(group.select(64).is_some());
        assert!(group.select(127).is_some());
    }

    #[test]
    fn select_velocity_mapping() {
        let soft = Arc::new(vec![0.1; 10]);
        let loud = Arc::new(vec![0.9; 10]);

        let group = NoteGroup {
            variants: vec![
                SampleVariant {
                    samples: Arc::clone(&soft),
                    velocity_layer: 1,
                    round_robin: 1,
                },
                SampleVariant {
                    samples: Arc::clone(&loud),
                    velocity_layer: 2,
                    round_robin: 1,
                },
            ],
            max_velocity_layer: 2,
            max_round_robin: 1,
            rr_counter: AtomicUsize::new(0),
        };

        // Low velocity → layer 1 (soft)
        let s = group.select(1).unwrap();
        assert_eq!(s[0], 0.1);

        // Mid velocity → still layer 1
        let s = group.select(63).unwrap();
        assert_eq!(s[0], 0.1);

        // Higher velocity → layer 2 (loud)
        let s = group.select(64).unwrap();
        assert_eq!(s[0], 0.9);

        // Max velocity → layer 2 (loud)
        let s = group.select(127).unwrap();
        assert_eq!(s[0], 0.9);
    }

    #[test]
    fn select_round_robin_cycling() {
        let rr1 = Arc::new(vec![1.0; 10]);
        let rr2 = Arc::new(vec![2.0; 10]);
        let rr3 = Arc::new(vec![3.0; 10]);

        let group = NoteGroup {
            variants: vec![
                SampleVariant {
                    samples: Arc::clone(&rr1),
                    velocity_layer: 1,
                    round_robin: 1,
                },
                SampleVariant {
                    samples: Arc::clone(&rr2),
                    velocity_layer: 1,
                    round_robin: 2,
                },
                SampleVariant {
                    samples: Arc::clone(&rr3),
                    velocity_layer: 1,
                    round_robin: 3,
                },
            ],
            max_velocity_layer: 1,
            max_round_robin: 3,
            rr_counter: AtomicUsize::new(0),
        };

        // Repeated calls at same velocity should cycle through RR variants
        assert_eq!(group.select(64).unwrap()[0], 1.0);
        assert_eq!(group.select(64).unwrap()[0], 2.0);
        assert_eq!(group.select(64).unwrap()[0], 3.0);
        // Wraps around
        assert_eq!(group.select(64).unwrap()[0], 1.0);
    }

    #[test]
    fn load_kit_velocity_round_robin_grouping() {
        let dir = tempfile::tempdir().unwrap();
        // Note 38 with 2 velocity layers x 2 round-robins
        write_test_wav_value(&dir.path().join("38_v1_rr1.wav"), 44100, 2, 100);
        write_test_wav_value(&dir.path().join("38_v1_rr2.wav"), 44100, 2, 200);
        write_test_wav_value(&dir.path().join("38_v2_rr1.wav"), 44100, 2, 300);
        write_test_wav_value(&dir.path().join("38_v2_rr2.wav"), 44100, 2, 400);
        // Note 36 as a single sample
        write_test_wav(&dir.path().join("36.wav"), 44100, 2);

        let kit = load_kit(dir.path()).unwrap();
        assert_eq!(kit.notes.len(), 2);

        let group38 = &kit.notes[&38];
        assert_eq!(group38.variants.len(), 4);
        assert_eq!(group38.max_velocity_layer, 2);
        assert_eq!(group38.max_round_robin, 2);

        let group36 = &kit.notes[&36];
        assert_eq!(group36.variants.len(), 1);
        assert_eq!(group36.max_velocity_layer, 1);
        assert_eq!(group36.max_round_robin, 1);
    }

    #[test]
    fn select_velocity_with_three_layers() {
        let l1 = Arc::new(vec![1.0; 10]);
        let l2 = Arc::new(vec![2.0; 10]);
        let l3 = Arc::new(vec![3.0; 10]);

        let group = NoteGroup {
            variants: vec![
                SampleVariant {
                    samples: Arc::clone(&l1),
                    velocity_layer: 1,
                    round_robin: 1,
                },
                SampleVariant {
                    samples: Arc::clone(&l2),
                    velocity_layer: 2,
                    round_robin: 1,
                },
                SampleVariant {
                    samples: Arc::clone(&l3),
                    velocity_layer: 3,
                    round_robin: 1,
                },
            ],
            max_velocity_layer: 3,
            max_round_robin: 1,
            rr_counter: AtomicUsize::new(0),
        };

        // 3 layers: vel 1-42 → layer 1, vel 43-84 → layer 2, vel 85-127 → layer 3
        assert_eq!(group.select(1).unwrap()[0], 1.0);
        assert_eq!(group.select(42).unwrap()[0], 1.0);
        assert_eq!(group.select(43).unwrap()[0], 2.0);
        assert_eq!(group.select(84).unwrap()[0], 2.0);
        assert_eq!(group.select(85).unwrap()[0], 3.0);
        assert_eq!(group.select(127).unwrap()[0], 3.0);
    }
}
