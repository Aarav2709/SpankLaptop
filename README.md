# SpankLaptop
Hit your laptop. It yells back.

SpankLaptop is a Rust rewrite (for windows and linux ofc) inspired by
https://github.com/taigrr/spank!

Instead of using an accelerometer (which is present in MacOS), this version detects impacts through microphone vibration and plays a random sound in response.

Making this thing took me like 12-13 hours, with a lot of trial and testing, so I hope you enjoy using this and give me proper feedback as well :D

## Features

- Detects impacts using microphone input
- Plays a random sound from a selected sound pack
- Multiple built in modes
- Custom folder support for sound packs
- Adjustable sensitivity and cooldown

## Installation

Clone the repository and build with Cargo.

```bash
git clone https://github.com/Aarav2709/SpankLaptop
cd SpankLaptop
cargo build --release
```

Run the compiled binary:

```bash
./target/release/spanklaptop
```

## Usage

### Interactive mode

```bash
spanklaptop
```

### Run with a mode

```bash
spanklaptop --mode sexy
spanklaptop --mode halo
```

## Options

### --mode

Selects the sound pack used when an impact is detected.

Examples:

```bash
spanklaptop --mode pain
spanklaptop --mode sexy
spanklaptop --mode halo
```

You can also point to a custom directory:

```bash
spanklaptop --mode ./sounds/custom
```

### --min-amplitude

Sets the minimum amplitude required to trigger a sound.

Higher values make the detector less sensitive.

```bash
spanklaptop --min-amplitude 0.6
```

### --cooldown

Minimum time between triggers in milliseconds.

```bash
spanklaptop --cooldown 600
```

## Sound Packs

Audio files are loaded from a folder corresponding to the selected mode.

Example structure:

```
audio/
  pain/
    sound1.wav
    sound2.wav
  sexy/
    sound1.mp3
  halo/
    sound1.wav
```

Supported formats depend on `rodio` and typically include:

- WAV
- MP3
- OGG
