use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::BufferSize;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_output_devices_does_not_error() {
        // Should not panic or error, even if no devices are present
        let result = list_output_devices();
        assert!(result.is_ok());
    }
}
