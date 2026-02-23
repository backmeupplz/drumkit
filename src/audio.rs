use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::BufferSize;
use rtrb::Consumer;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// An audio output device descriptor
#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub name: String,
    pub index: usize,
}

/// List available audio output devices.
pub fn list_output_devices() -> Result<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

    for (i, device) in host.output_devices().context("Failed to enumerate audio output devices")?.enumerate() {
        let name = device.name().unwrap_or_else(|_| format!("Unknown device {}", i));
        devices.push(AudioDevice { name, index: i });
    }

    Ok(devices)
}

/// Get an output device by index, or the default if None.
fn get_device(device_index: Option<usize>) -> Result<cpal::Device> {
    let host = cpal::default_host();

    match device_index {
        Some(idx) => {
            let device = host
                .output_devices()
                .context("Failed to enumerate audio output devices")?
                .nth(idx)
                .context(format!("Audio device index {} not found", idx))?;
            Ok(device)
        }
        None => host
            .default_output_device()
            .context("No default audio output device found"),
    }
}

/// Play pre-decoded f32 PCM samples through a cpal output device.
/// Blocks until playback completes.
pub fn play_sample(
    device_index: Option<usize>,
    samples: Arc<Vec<f32>>,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    let device = get_device(device_index)?;
    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: BufferSize::Fixed(64),
    };

    let position = Arc::new(AtomicUsize::new(0));
    let total_samples = samples.len();

    let pos = Arc::clone(&position);
    let data = Arc::clone(&samples);

    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();

    let stream = device
        .build_output_stream(
            &config,
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let current = pos.load(Ordering::Relaxed);
                let remaining = total_samples.saturating_sub(current);
                let to_copy = output.len().min(remaining);

                if to_copy > 0 {
                    output[..to_copy].copy_from_slice(&data[current..current + to_copy]);
                }
                // Fill the rest with silence
                for sample in output[to_copy..].iter_mut() {
                    *sample = 0.0;
                }

                let new_pos = current + to_copy;
                pos.store(new_pos, Ordering::Relaxed);

                if new_pos >= total_samples {
                    let _ = done_tx.send(());
                }
            },
            move |err| {
                eprintln!("Audio stream error: {}", err);
            },
            None,
        )
        .with_context(|| format!("Failed to build audio stream on {}", device_name))?;

    stream
        .play()
        .with_context(|| format!("Failed to start audio stream on {}", device_name))?;

    println!("Playing on: {}", device_name);

    // Wait for playback to complete
    let _ = done_rx.recv();

    // Small drain delay to let the last buffer flush
    std::thread::sleep(std::time::Duration::from_millis(50));

    Ok(())
}

/// Command sent from the MIDI thread to the audio thread via rtrb.
pub enum AudioCommand {
    /// Trigger a new voice with the given samples, gain, and source note.
    Trigger {
        samples: Arc<Vec<f32>>,
        gain: f32,
        note: u8,
    },
    /// Choke (fade out) all playing voices for the given note.
    Choke {
        note: u8,
        fade_frames: usize,
    },
}

/// Fade-out state for a voice being choked.
struct Fade {
    remaining_frames: usize,
    total_frames: usize,
}

/// A single active playback voice in the mixer.
pub struct Voice {
    pub samples: Arc<Vec<f32>>,
    pub position: usize,
    pub gain: f32,
    pub note: u8,
    fade: Option<Fade>,
}

impl Voice {
    /// Returns true when this voice has finished playing all its samples
    /// or has completed its fade-out.
    pub fn is_done(&self) -> bool {
        self.position >= self.samples.len()
            || self.fade.as_ref().is_some_and(|f| f.remaining_frames == 0)
    }

    /// Start a fade-out over the given number of frames.
    /// If already fading, keeps the shorter remaining fade.
    fn start_fade(&mut self, fade_frames: usize) {
        match &self.fade {
            Some(existing) if existing.remaining_frames <= fade_frames => {}
            _ => {
                self.fade = Some(Fade {
                    remaining_frames: fade_frames,
                    total_frames: fade_frames,
                });
            }
        }
    }

    /// Compute effective gain for this frame, advancing fade state if active.
    fn frame_gain(&mut self) -> f32 {
        if let Some(ref mut fade) = self.fade {
            if fade.remaining_frames == 0 {
                return 0.0;
            }
            let factor = fade.remaining_frames as f32 / fade.total_frames as f32;
            fade.remaining_frames -= 1;
            self.gain * factor
        } else {
            self.gain
        }
    }
}

const MAX_POLYPHONY: usize = 16;

/// Start a persistent audio output stream that mixes voices triggered via rtrb.
///
/// The returned `cpal::Stream` must be kept alive — dropping it stops audio.
pub fn run_output_stream(
    device_index: Option<usize>,
    mut consumer: Consumer<AudioCommand>,
    sample_rate: u32,
    channels: u16,
) -> Result<cpal::Stream> {
    let device = get_device(device_index)?;
    let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

    let config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: BufferSize::Fixed(64),
    };

    let ch = channels as usize;
    let mut voices: Vec<Voice> = Vec::with_capacity(MAX_POLYPHONY);

    let stream = device
        .build_output_stream(
            &config,
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                // Drain all pending commands from the ring buffer
                while let Ok(cmd) = consumer.pop() {
                    match cmd {
                        AudioCommand::Trigger {
                            samples,
                            gain,
                            note,
                        } => {
                            if voices.len() < MAX_POLYPHONY {
                                voices.push(Voice {
                                    samples,
                                    position: 0,
                                    gain,
                                    note,
                                    fade: None,
                                });
                            }
                            // Excess triggers silently dropped
                        }
                        AudioCommand::Choke { note, fade_frames } => {
                            for voice in voices.iter_mut() {
                                if voice.note == note {
                                    voice.start_fade(fade_frames);
                                }
                            }
                        }
                    }
                }

                // Zero the output buffer
                for sample in output.iter_mut() {
                    *sample = 0.0;
                }

                // Mix all active voices into the output
                let frames = output.len() / ch;
                for voice in &mut voices {
                    for frame in 0..frames {
                        if voice.is_done() {
                            break;
                        }
                        let gain = voice.frame_gain();
                        for c in 0..ch {
                            if voice.position + c < voice.samples.len() {
                                output[frame * ch + c] +=
                                    voice.samples[voice.position + c] * gain;
                            }
                        }
                        voice.position += ch;
                    }
                }

                // Remove finished voices
                voices.retain(|v| !v.is_done());
            },
            move |err| {
                eprintln!("Audio stream error: {}", err);
            },
            None,
        )
        .with_context(|| format!("Failed to build audio stream on {}", device_name))?;

    stream
        .play()
        .with_context(|| format!("Failed to start audio stream on {}", device_name))?;

    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_output_devices_does_not_error() {
        // Should not panic or error, even if no devices are present
        let result = list_output_devices();
        assert!(result.is_ok());
    }

    #[test]
    fn voice_is_done_at_end() {
        let samples = Arc::new(vec![0.0_f32; 100]);
        let voice = Voice {
            samples: Arc::clone(&samples),
            position: 100,
            gain: 1.0,
            note: 36,
            fade: None,
        };
        assert!(voice.is_done());
    }

    #[test]
    fn voice_is_not_done_mid_playback() {
        let samples = Arc::new(vec![0.0_f32; 100]);
        let voice = Voice {
            samples: Arc::clone(&samples),
            position: 50,
            gain: 1.0,
            note: 36,
            fade: None,
        };
        assert!(!voice.is_done());
    }

    #[test]
    fn voice_is_done_after_fade() {
        let samples = Arc::new(vec![0.0_f32; 1000]);
        let voice = Voice {
            samples: Arc::clone(&samples),
            position: 0,
            gain: 1.0,
            note: 46,
            fade: Some(Fade {
                remaining_frames: 0,
                total_frames: 100,
            }),
        };
        assert!(voice.is_done());
    }

    #[test]
    fn voice_fade_ramps_gain_to_zero() {
        let samples = Arc::new(vec![0.0_f32; 1000]);
        let mut voice = Voice {
            samples: Arc::clone(&samples),
            position: 0,
            gain: 1.0,
            note: 46,
            fade: None,
        };

        // No fade — full gain
        assert!((voice.frame_gain() - 1.0).abs() < f32::EPSILON);

        // Start 4-frame fade
        voice.start_fade(4);

        // Frame gains should ramp down: 4/4, 3/4, 2/4, 1/4, then 0
        let g1 = voice.frame_gain();
        assert!((g1 - 1.0).abs() < f32::EPSILON); // 4/4
        let g2 = voice.frame_gain();
        assert!((g2 - 0.75).abs() < f32::EPSILON); // 3/4
        let g3 = voice.frame_gain();
        assert!((g3 - 0.5).abs() < f32::EPSILON); // 2/4
        let g4 = voice.frame_gain();
        assert!((g4 - 0.25).abs() < f32::EPSILON); // 1/4
        let g5 = voice.frame_gain();
        assert!((g5 - 0.0).abs() < f32::EPSILON); // done
        assert!(voice.is_done());
    }

    #[test]
    fn audio_command_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AudioCommand>();
    }

    #[test]
    fn audio_command_roundtrips_through_rtrb() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(4);
        let samples = Arc::new(vec![1.0_f32, 2.0, 3.0]);
        producer
            .push(AudioCommand::Trigger {
                samples: Arc::clone(&samples),
                gain: 0.5,
                note: 38,
            })
            .unwrap();

        let cmd = consumer.pop().unwrap();
        match cmd {
            AudioCommand::Trigger { samples: s, gain, note } => {
                assert_eq!(s.len(), 3);
                assert!((gain - 0.5).abs() < f32::EPSILON);
                assert_eq!(note, 38);
            }
            _ => panic!("Expected Trigger command"),
        }
    }

    #[test]
    fn choke_command_roundtrips_through_rtrb() {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(4);
        producer
            .push(AudioCommand::Choke {
                note: 46,
                fade_frames: 3000,
            })
            .unwrap();

        let cmd = consumer.pop().unwrap();
        match cmd {
            AudioCommand::Choke { note, fade_frames } => {
                assert_eq!(note, 46);
                assert_eq!(fade_frames, 3000);
            }
            _ => panic!("Expected Choke command"),
        }
    }
}
