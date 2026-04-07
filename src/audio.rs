use rand::seq::SliceRandom;
use rodio::{Decoder, OutputStream, Sink};
use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::PathBuf,
    sync::mpsc::Receiver,
};

use include_dir::{Dir, include_dir};

static AUDIO_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/audio");

pub enum SoundSource {
    File(PathBuf),
    Embedded(&'static [u8]),
}

pub fn load(mode_or_path: &str) -> anyhow::Result<Vec<SoundSource>> {
    if matches!(mode_or_path, "pain" | "sexy" | "halo") {
        return load_embedded_pack(mode_or_path);
    }

    load_from_directory(mode_or_path)
}

fn load_embedded_pack(mode: &str) -> anyhow::Result<Vec<SoundSource>> {
    let mut sounds = Vec::new();

    let pack = AUDIO_DIR
        .get_dir(mode)
        .ok_or_else(|| anyhow::anyhow!("Built-in pack '{}' not found", mode))?;

    for file in pack.files() {
        if let Some(ext) = file.path().extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("mp3") || ext.eq_ignore_ascii_case("wav") {
                sounds.push(SoundSource::Embedded(file.contents()));
            }
        }
    }

    if sounds.is_empty() {
        anyhow::bail!("No embedded audio files found in built-in pack '{}'.", mode);
    }

    Ok(sounds)
}

fn load_from_directory(dir: &str) -> anyhow::Result<Vec<SoundSource>> {
    let mut sounds = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();

        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("mp3") || ext.eq_ignore_ascii_case("wav") {
                sounds.push(SoundSource::File(path));
            }
        }
    }

    if sounds.is_empty() {
        anyhow::bail!("No audio files found in {}", dir);
    }

    Ok(sounds)
}

pub fn start_player(rx: Receiver<()>, sounds: Vec<SoundSource>) {
    std::thread::spawn(move || {
        let (_stream, handle) = OutputStream::try_default().unwrap();
        let mut rng = rand::thread_rng();

        loop {
            if rx.recv().is_ok() {
                if let Some(sound) = sounds.choose(&mut rng) {
                    match sound {
                        SoundSource::File(path) => {
                            if let Ok(file) = File::open(path) {
                                if let Ok(source) = Decoder::new(BufReader::new(file)) {
                                    let sink = Sink::try_new(&handle).unwrap();
                                    sink.append(source);
                                    sink.detach();
                                }
                            }
                        }
                        SoundSource::Embedded(bytes) => {
                            if let Ok(source) = Decoder::new(Cursor::new(*bytes)) {
                                let sink = Sink::try_new(&handle).unwrap();
                                sink.append(source);
                                sink.detach();
                            }
                        }
                    }
                }
            }
        }
    });
}
