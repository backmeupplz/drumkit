use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a mapping was loaded from.
#[derive(Debug, Clone, PartialEq)]
pub enum MappingSource {
    BuiltIn,
    UserFile(PathBuf),
    KitFile(PathBuf),
}

/// Raw TOML schema — keys are strings because TOML only supports string keys.
#[derive(Deserialize, Serialize)]
struct MappingFile {
    name: String,
    #[serde(default)]
    notes: HashMap<String, String>,
    #[serde(default)]
    chokes: HashMap<String, Vec<u8>>,
}

/// A mapping from MIDI note numbers to human-readable names and choke rules.
#[derive(Debug, Clone)]
pub struct NoteMapping {
    pub name: String,
    pub notes: HashMap<u8, String>,
    pub chokes: HashMap<u8, Vec<u8>>,
    pub source: MappingSource,
}

impl NoteMapping {
    /// Look up the name for a MIDI note, falling back to "Unknown".
    pub fn drum_name(&self, note: u8) -> &str {
        self.notes
            .get(&note)
            .map(|s| s.as_str())
            .unwrap_or("Unknown")
    }

    /// Look up the choke targets for a MIDI note (empty slice if none).
    pub fn choke_targets(&self, note: u8) -> &[u8] {
        self.chokes
            .get(&note)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Set or update the name for a MIDI note.
    pub fn set_note_name(&mut self, note: u8, name: String) {
        self.notes.insert(note, name);
    }
}

/// Parse a TOML string into a `NoteMapping`.
pub fn parse_mapping(toml_str: &str, source: MappingSource) -> Result<NoteMapping> {
    let file: MappingFile =
        toml::from_str(toml_str).context("Failed to parse mapping TOML")?;

    let mut notes = HashMap::new();
    for (k, v) in file.notes {
        let note: u8 = k.parse().with_context(|| format!("Invalid note number: {}", k))?;
        notes.insert(note, v);
    }

    let mut chokes = HashMap::new();
    for (k, v) in file.chokes {
        let note: u8 = k.parse().with_context(|| format!("Invalid choke note: {}", k))?;
        chokes.insert(note, v);
    }

    Ok(NoteMapping {
        name: file.name,
        notes,
        chokes,
        source,
    })
}

/// Serialize a `NoteMapping` back to TOML.
pub fn serialize_mapping(mapping: &NoteMapping) -> Result<String> {
    let mut notes = HashMap::new();
    for (&k, v) in &mapping.notes {
        notes.insert(k.to_string(), v.clone());
    }

    let mut chokes = HashMap::new();
    for (&k, v) in &mapping.chokes {
        chokes.insert(k.to_string(), v.clone());
    }

    let file = MappingFile {
        name: mapping.name.clone(),
        notes,
        chokes,
    };

    toml::to_string_pretty(&file).context("Failed to serialize mapping")
}

/// Hardcoded fallback mapping — guaranteed to always work even if TOML parsing
/// is somehow broken. Contains the core GM drum names and hi-hat choke rules.
fn fallback_mapping() -> NoteMapping {
    let notes: HashMap<u8, String> = [
        (21, "HH Splash"), (23, "HH Half-Open"), (35, "Bass Drum 2"),
        (36, "Kick"), (37, "Side Stick"), (38, "Snare"),
        (39, "Hand Clap"), (40, "Snare Rim"), (41, "Low Floor Tom"),
        (42, "Closed Hi-Hat"), (43, "High Floor Tom"), (44, "Pedal Hi-Hat"),
        (45, "Low Tom"), (46, "Open Hi-Hat"), (47, "Low-Mid Tom"),
        (48, "Hi-Mid Tom"), (49, "Crash 1"), (50, "High Tom"),
        (51, "Ride"), (52, "Chinese Cymbal"), (53, "Ride Bell"),
        (54, "Tambourine"), (55, "Splash Cymbal"), (56, "Cowbell"),
        (57, "Crash 2"), (58, "Vibraslap"), (59, "Ride 2"),
        (60, "Hi Bongo"), (61, "Low Bongo"), (62, "Mute Hi Conga"),
        (63, "Open Hi Conga"), (64, "Low Conga"), (65, "High Timbale"),
        (66, "Low Timbale"), (67, "High Agogo"), (68, "Low Agogo"),
        (69, "Cabasa"), (70, "Maracas"), (71, "Short Whistle"),
        (72, "Long Whistle"), (73, "Short Guiro"), (74, "Long Guiro"),
        (75, "Claves"), (76, "Hi Wood Block"), (77, "Low Wood Block"),
        (78, "Mute Cuica"), (79, "Open Cuica"), (80, "Mute Triangle"),
        (81, "Open Triangle"),
    ].into_iter().map(|(n, s)| (n, s.to_string())).collect();

    let chokes: HashMap<u8, Vec<u8>> = [
        (42, vec![46, 23, 21]),
        (44, vec![46, 23, 21]),
        (23, vec![46]),
    ].into_iter().collect();

    NoteMapping {
        name: "General MIDI".to_string(),
        notes,
        chokes,
        source: MappingSource::BuiltIn,
    }
}

/// Return the default mapping (General MIDI). Parses the built-in TOML preset,
/// falling back to a hardcoded mapping if parsing fails.
pub fn default_mapping() -> NoteMapping {
    let gm_toml = include_str!("../mappings/general-midi.toml");
    parse_mapping(gm_toml, MappingSource::BuiltIn).unwrap_or_else(|_| fallback_mapping())
}

/// Try to load a kit-specific mapping from `<kit_path>/mapping.toml`.
/// Returns `None` if the file doesn't exist or fails to parse.
pub fn load_kit_mapping(kit_path: &Path) -> Option<NoteMapping> {
    let mapping_path = kit_path.join("mapping.toml");
    let content = std::fs::read_to_string(&mapping_path).ok()?;
    parse_mapping(&content, MappingSource::KitFile(mapping_path)).ok()
}

/// Return the built-in preset mappings compiled into the binary.
pub fn builtin_mappings() -> Vec<NoteMapping> {
    let gm_toml = include_str!("../mappings/general-midi.toml");
    let alesis_toml = include_str!("../mappings/alesis-nitro-max.toml");

    let mut mappings = Vec::new();
    mappings.push(
        parse_mapping(gm_toml, MappingSource::BuiltIn).unwrap_or_else(|_| fallback_mapping()),
    );
    if let Ok(m) = parse_mapping(alesis_toml, MappingSource::BuiltIn) {
        mappings.push(m);
    }
    mappings
}

/// Return the XDG data directory for user mappings.
pub fn user_mappings_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local/share")
        });
    base.join("drumkit/mappings")
}

/// Discover user mapping files from the XDG data directory and any extra directories.
pub fn discover_user_mappings(extra_dirs: &[PathBuf]) -> Vec<NoteMapping> {
    let mut dirs_to_scan = vec![user_mappings_dir()];
    dirs_to_scan.extend(extra_dirs.iter().cloned());

    let mut mappings = Vec::new();
    for dir in &dirs_to_scan {
        if !dir.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(m) = parse_mapping(&content, MappingSource::UserFile(path)) {
                            mappings.push(m);
                        }
                    }
                }
            }
        }
    }
    mappings.sort_by(|a, b| a.name.cmp(&b.name));
    mappings
}

/// Return all available mappings: built-in presets followed by user mappings.
pub fn discover_all_mappings(extra_dirs: &[PathBuf]) -> Vec<NoteMapping> {
    let mut all = builtin_mappings();
    all.extend(discover_user_mappings(extra_dirs));
    all
}

/// Save a user mapping to the XDG data directory. Returns the file path.
pub fn save_user_mapping(mapping: &NoteMapping) -> Result<PathBuf> {
    let dir = user_mappings_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create mappings dir: {}", dir.display()))?;

    let filename = mapping
        .name
        .to_lowercase()
        .replace(' ', "-")
        .replace(|c: char| !c.is_alphanumeric() && c != '-', "");
    let path = dir.join(format!("{}.toml", filename));

    let content = serialize_mapping(mapping)?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write mapping: {}", path.display()))?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_mapping() {
        let toml = r#"
name = "Test"

[notes]
36 = "Kick"
38 = "Snare"

[chokes]
42 = [46, 23]
"#;
        let m = parse_mapping(toml, MappingSource::BuiltIn).unwrap();
        assert_eq!(m.name, "Test");
        assert_eq!(m.drum_name(36), "Kick");
        assert_eq!(m.drum_name(38), "Snare");
        assert_eq!(m.drum_name(99), "Unknown");
        assert_eq!(m.choke_targets(42), &[46, 23]);
        assert!(m.choke_targets(99).is_empty());
    }

    #[test]
    fn parse_empty_sections() {
        let toml = r#"name = "Empty""#;
        let m = parse_mapping(toml, MappingSource::BuiltIn).unwrap();
        assert_eq!(m.name, "Empty");
        assert!(m.notes.is_empty());
        assert!(m.chokes.is_empty());
    }

    #[test]
    fn parse_invalid_note_number() {
        let toml = r#"
name = "Bad"

[notes]
999 = "TooHigh"
"#;
        // 999 doesn't fit in u8
        let result = parse_mapping(toml, MappingSource::BuiltIn);
        assert!(result.is_err());
    }

    #[test]
    fn serialize_roundtrip() {
        let original = NoteMapping {
            name: "Roundtrip".to_string(),
            notes: [(36, "Kick".to_string()), (38, "Snare".to_string())]
                .into_iter()
                .collect(),
            chokes: [(42, vec![46, 23])].into_iter().collect(),
            source: MappingSource::BuiltIn,
        };

        let serialized = serialize_mapping(&original).unwrap();
        let parsed = parse_mapping(&serialized, MappingSource::BuiltIn).unwrap();

        assert_eq!(parsed.name, original.name);
        assert_eq!(parsed.drum_name(36), "Kick");
        assert_eq!(parsed.drum_name(38), "Snare");
        assert_eq!(parsed.choke_targets(42), &[46, 23]);
    }

    #[test]
    fn set_note_name() {
        let mut m = NoteMapping {
            name: "Test".to_string(),
            notes: HashMap::new(),
            chokes: HashMap::new(),
            source: MappingSource::BuiltIn,
        };
        assert_eq!(m.drum_name(60), "Unknown");
        m.set_note_name(60, "Custom".to_string());
        assert_eq!(m.drum_name(60), "Custom");
    }

    #[test]
    fn builtin_mappings_load() {
        let mappings = builtin_mappings();
        assert_eq!(mappings.len(), 2);
        assert_eq!(mappings[0].name, "General MIDI");
        assert_eq!(mappings[1].name, "Alesis Nitro Max");

        // Spot-check GM
        assert_eq!(mappings[0].drum_name(36), "Kick");
        assert_eq!(mappings[0].drum_name(38), "Snare");
        assert_eq!(mappings[0].choke_targets(42), &[46, 23, 21]);

        // Spot-check Alesis
        assert_eq!(mappings[1].drum_name(40), "Snare (Rim)");
        assert_eq!(mappings[1].drum_name(58), "Tom 3 (Rim)");
    }

    #[test]
    fn save_and_read_back_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let mappings_dir = dir.path().join("drumkit/mappings");
        std::fs::create_dir_all(&mappings_dir).unwrap();

        let mapping = NoteMapping {
            name: "My Custom Kit".to_string(),
            notes: [(36, "Kick".to_string())].into_iter().collect(),
            chokes: HashMap::new(),
            source: MappingSource::BuiltIn,
        };

        let content = serialize_mapping(&mapping).unwrap();
        let path = mappings_dir.join("my-custom-kit.toml");
        std::fs::write(&path, &content).unwrap();

        // Read it back
        let read_content = std::fs::read_to_string(&path).unwrap();
        let parsed = parse_mapping(&read_content, MappingSource::UserFile(path)).unwrap();
        assert_eq!(parsed.name, "My Custom Kit");
        assert_eq!(parsed.drum_name(36), "Kick");
    }

    #[test]
    fn load_kit_mapping_found() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = "name = \"Test Kit\"\n\n[notes]\n62 = \"Conga\"\n";
        std::fs::write(dir.path().join("mapping.toml"), toml_content).unwrap();

        let result = load_kit_mapping(dir.path());
        assert!(result.is_some());
        let m = result.unwrap();
        assert_eq!(m.name, "Test Kit");
        assert_eq!(m.drum_name(62), "Conga");
        assert!(matches!(m.source, MappingSource::KitFile(_)));
    }

    #[test]
    fn load_kit_mapping_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_kit_mapping(dir.path()).is_none());
    }
}
