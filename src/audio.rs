use rand::seq::SliceRandom;
use rodio::{Decoder, OutputStream, Sink};
use std::{
    fs::File,
    io::BufReader,
    sync::mpsc::Receiver,
};

pub fn load(dir: &str) -> anyhow::Result<Vec<String>> {
    let mut sounds = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();

        if let Some(ext) = path.extension() {
            if ext == "mp3" || ext == "wav" {
                sounds.push(path.to_string_lossy().to_string());
            }
        }
    }

    if sounds.is_empty() {
        anyhow::bail!("No audio files found in {}", dir);
    }

    Ok(sounds)
}

pub fn start_player(rx: Receiver<()>, sounds: Vec<String>) {
    std::thread::spawn(move || {
        let (_stream, handle) = OutputStream::try_default().unwrap();
        let mut rng = rand::thread_rng();

        loop {
            if rx.recv().is_ok() {
                if let Some(path) = sounds.choose(&mut rng) {
                    if let Ok(file) = File::open(path) {
                        if let Ok(source) = Decoder::new(BufReader::new(file)) {
                            let sink = Sink::try_new(&handle).unwrap();
                            sink.append(source);
                            sink.detach();
                        }
                    }
                }
            }
        }
    });
}
