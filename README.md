# SpankLaptop

> Hit your laptop. It yells back.

SpankLaptop is a Rust rewrite (for windows and linux ofc) inspired by: https://github.com/taigrr/spank

It listens for impacts through your microphone, and through the accelerometer on laptops that have one, then plays a random sound in response.
Making this thing took me like 12-13 hours, with a lot of trial and testing, so I hope you enjoy using this and give me proper feedback as well :D

https://github.com/user-attachments/assets/5964abaa-ee9a-4aaf-853c-9984433d8a08
> I'm not going to lie, this is super duper old, and I'm lazy to show the new one to you, so just check it out yourself <3

## Features

- Detects hits with the microphone, plus the accelerometer when your laptop has one
- Built in packs: Pain and Halo, or point it at your own folder (mp3, wav, ogg, flac)
- Harder hits play louder, and quick hits in a row build a combo that speeds the sound up
- Live dashboard with a level meter, hit stats and hotkeys
- Calibration that learns how hard you hit
- Runs in the system tray and can start when you log in
- Remembers your settings and keeps hit stats (today, all time, hardest hit, best combo)
- Reconnects on its own if the mic is unplugged
- Ignores its own sounds, so it never triggers itself

## Install

Grab a binary from the [releases page](https://github.com/Aarav2709/SpankLaptop/releases), or build it:

```
cargo install SpankLaptop
```

On Linux you need the ALSA headers first (`sudo apt install libasound2-dev pkg-config` on Debian and Ubuntu).

### Flags

| Flag | Values | Default |
| --- | --- | --- |
| `-m`, `--mode` | `pain`, `halo`, or a folder path | menu |
| `-s`, `--sensitivity` | `1` to `10`, each step is twice as sensitive | `5` |
| `-c`, `--cooldown` | milliseconds between sounds | `450` |
| `--mic` | part of a mic name, see `--list-mics` | system default |
| `--source` | `auto`, `mic` or `accel` | `auto` |
| `--calibrate` | calibrate on startup | |
| `--plain` | scrolling text instead of the dashboard | |
| `--tray` | run in the background with a tray icon | |
| `--stats` | print your hit stats | |
| `--install-startup` / `--uninstall-startup` | start in the tray at login | |

Flags only apply to that run. Changes you make with hotkeys, the menu or calibration are saved.

### It never triggers

On Windows, many laptop mics run noise suppression that squashes knocks. Turn it off in Settings, System, Sound, your microphone, Audio enhancements: Off.

### It triggers too easily

Press `←` until typing and talking stop turning the bars orange.

### Config file

Settings live in `config.toml` and stats in `stats.toml`:

- Windows: `%APPDATA%\SpankLaptop`
- Linux: `~/.config/spanklaptop`

```toml
mode = "pain"            # pain, halo or custom
custom_folder = "C:\\sounds"
sensitivity = 5.0
cooldown_ms = 450
mic = "USB"              # part of the mic name, leave out for the default
source = "auto"          # auto, mic or accel
volume = 1.0
dynamic_volume = true    # harder hits play louder
combo = true
```

### Tray

`SpankLaptop --tray` runs it in the background. The tray menu can pause it, switch packs, toggle start at login and quit. On Linux the tray needs a desktop with StatusNotifierItem support (KDE, or GNOME with the AppIndicator extension); without one it keeps running without an icon.

## Accelerometer support

- Windows: any sensor exposed through the Windows sensor API, common on 2 in 1 laptops and tablets
- Linux: IIO accelerometers under `/sys/bus/iio/devices`

Most regular laptops don't have one, in which case only the mic is used.
