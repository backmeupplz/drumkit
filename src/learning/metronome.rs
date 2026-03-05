use std::sync::Arc;

/// Generate a metronome click sample (sine burst with exponential decay).
///
/// - `freq_hz`: Pitch of the click (1000 Hz for downbeat, 800 Hz for off-beat)
/// - `sample_rate`: Audio sample rate
/// - `channels`: Number of audio channels (samples are duplicated across channels)
/// - `duration_ms`: Click duration in milliseconds (typically ~30ms)
fn generate_click(freq_hz: f64, sample_rate: u32, channels: u16, duration_ms: u32) -> Arc<Vec<f32>> {
    let num_frames = (sample_rate as u64 * duration_ms as u64 / 1000) as usize;
    let ch = channels as usize;
    let mut samples = Vec::with_capacity(num_frames * ch);
    let decay_rate = 5.0 / (num_frames as f64); // ~5 time constants over duration

    for frame in 0..num_frames {
        let t = frame as f64 / sample_rate as f64;
        let envelope = (-decay_rate * frame as f64).exp();
        let value = (2.0 * std::f64::consts::PI * freq_hz * t).sin() * envelope * 0.5;
        let sample = value as f32;
        for _ in 0..ch {
            samples.push(sample);
        }
    }

    Arc::new(samples)
}

/// Generate the downbeat (strong beat) click.
pub fn downbeat_click(sample_rate: u32, channels: u16) -> Arc<Vec<f32>> {
    generate_click(1000.0, sample_rate, channels, 30)
}

/// Generate the off-beat (weak beat) click.
pub fn offbeat_click(sample_rate: u32, channels: u16) -> Arc<Vec<f32>> {
    generate_click(800.0, sample_rate, channels, 30)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_has_correct_length() {
        let click = generate_click(1000.0, 44100, 2, 30);
        // 30ms at 44100 Hz, stereo = 1323 frames * 2 = 2646 samples
        let expected_frames = (44100 * 30 / 1000) as usize;
        assert_eq!(click.len(), expected_frames * 2);
    }

    #[test]
    fn click_starts_louder_than_end() {
        let click = generate_click(1000.0, 44100, 1, 30);
        let start_energy: f32 = click[..10].iter().map(|s| s * s).sum();
        let end_energy: f32 = click[click.len() - 10..].iter().map(|s| s * s).sum();
        assert!(start_energy > end_energy);
    }
}
