# drumkit

Low-latency TUI MIDI drum sampler for electronic drum kits on Linux.

Connect your e-drums (Alesis Nitro Max, Roland, Yamaha, etc.) via USB and trigger custom audio samples with minimal latency. Manage sample libraries with a simple folder structure — no DAW required.

## Features

- **Ultra-low latency** — lock-free audio pipeline targeting <6ms pad-to-sound
- **Simple sample mapping** — drop WAV/MP3/FLAC files named by MIDI note number into a folder
- **Velocity layers** — `38_v1.wav`, `38_v2.wav` for dynamic expression
- **Round-robin** — `38_v1_rr1.wav`, `38_v1_rr2.wav` to avoid machine-gun effect
- **Hot-reload** — edit the library folder while playing, changes load automatically
- **TUI interface** — select devices, switch kits, visualize hits in the terminal
- **Pre-loaded samples** — everything decoded and held in RAM for zero-latency playback

## Install

### From AUR (Arch Linux)

```
yay -S drumkit
```

### From source

```
cargo install --path .
```

### Build dependencies

- Rust 1.75+
- ALSA development libraries: `sudo pacman -S alsa-lib` (Arch) or `sudo apt install libasound2-dev` (Debian/Ubuntu)

## Usage

### List MIDI devices

```
drumkit devices
```

### Monitor MIDI input

Connect your drum kit via USB and see what MIDI messages it sends:

```
drumkit monitor --port 0
```

Hit your pads to see note numbers, velocities, and drum names in real-time. Use this to verify your kit is working and to find the correct MIDI note numbers for sample mapping.

## Sample Library Structure

Samples live in `~/.config/drumkit/library/`. Each subdirectory is a kit:

```
~/.config/drumkit/library/
├── My-Rock-Kit/
│   ├── 36.wav              # Kick — single sample
│   ├── 38_v1_rr1.wav       # Snare — soft, variation 1
│   ├── 38_v1_rr2.wav       # Snare — soft, variation 2
│   ├── 38_v2_rr1.wav       # Snare — hard, variation 1
│   ├── 38_v2_rr2.wav       # Snare — hard, variation 2
│   ├── 42.wav              # Closed hi-hat
│   ├── 46.wav              # Open hi-hat
│   └── 49.wav              # Crash
└── Electronic-Kit/
    └── ...
```

### Naming convention

`{midi_note}[_v{velocity_layer}][_rr{round_robin}].{ext}`

| Pattern | Meaning |
|---|---|
| `36.wav` | Single sample for MIDI note 36 (kick) |
| `38_rr1.wav`, `38_rr2.wav` | Round-robin only (cycles through variations) |
| `38_v1.wav`, `38_v2.wav` | Velocity layers only (v1=soft, v2=hard) |
| `38_v1_rr1.wav` | Both velocity layers and round-robin |

### Common MIDI note numbers (General MIDI / Alesis Nitro Max)

| Note | Drum |
|---|---|
| 36 | Kick |
| 38 | Snare (head) |
| 40 | Snare (rim) |
| 42 | Closed Hi-Hat |
| 44 | Pedal Hi-Hat |
| 46 | Open Hi-Hat |
| 48 | Hi-Mid Tom |
| 45 | Low Tom |
| 43 | High Floor Tom |
| 41 | Low Floor Tom |
| 49 | Crash 1 |
| 57 | Crash 2 |
| 51 | Ride |

## Architecture

```
[MIDI Thread]  →  lock-free ring buffer  →  [Audio Thread (RT)]  →  speakers
                                                    ↑
[Filesystem Watcher]  →  [Main Thread / TUI]  →  sample reload
```

- MIDI input via `midir` (ALSA/JACK backends)
- Audio output via `cpal` (ALSA/PipeWire)
- Lock-free SPSC ring buffer between threads (`rtrb`)
- Samples pre-decoded to f32 PCM in RAM
- Zero allocations on the audio thread
- Filesystem watching via `notify` for hot-reload

## Low-Latency Tips

For the best experience on Arch Linux:

```bash
# PipeWire low-latency config
mkdir -p ~/.config/pipewire/pipewire.conf.d
cat > ~/.config/pipewire/pipewire.conf.d/low-latency.conf << 'EOF'
context.properties = {
    default.clock.rate = 48000
    default.clock.quantum = 64
    default.clock.min-quantum = 32
}
EOF
systemctl --user restart pipewire
```

## Roadmap

- [x] Stage 1: MIDI monitor
- [ ] Stage 2: Single sample playback
- [ ] Stage 3: MIDI-triggered sample playback
- [ ] Stage 4: Library loading and mapping
- [ ] Stage 5: Velocity layers and round-robin
- [ ] Stage 6: Hot-reload on library changes
- [ ] Stage 7: TUI interface
- [ ] Stage 8: AUR package
- [ ] Bundled sample kits (FreePats GM, AVL Drumkits, Virtuosity Drums)

## License

MIT
