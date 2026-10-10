use anyhow::Context;
use rand::{Rng, rngs::ThreadRng};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    fs::File,
    io::{BufReader, Cursor, Read, Seek},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use include_dir::{Dir, include_dir};

static AUDIO_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/audio");

pub const BUILT_IN: [&str; 2] = ["pain", "halo"];

const EXTENSIONS: [&str; 4] = ["mp3", "wav", "ogg", "flac"];

// longer files are streamed from disk instead of kept in memory
const MAX_PRELOAD_SECS: usize = 20;

type BoxedSource = Box<dyn Source<Item = i16> + Send>;

enum Clip {
    Memory {
        channels: u16,
        rate: u32,
        samples: Arc<[i16]>,
    },
    File(PathBuf),
}

impl Clip {
    fn source(&self) -> anyhow::Result<BoxedSource> {
        Ok(match self {
            Clip::Memory {
                channels,
                rate,
                samples,
            } => Box::new(MemorySource {
                samples: samples.clone(),
                pos: 0,
                channels: *channels,
                rate: *rate,
            }),
            Clip::File(path) => Box::new(Decoder::new(BufReader::new(File::open(path)?))?),
        })
    }
}

// decodes the whole clip up front so hits play instantly
fn preload<R>(reader: R, limit: Option<usize>) -> anyhow::Result<Option<Clip>>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let decoder = Decoder::new(reader)?;
    let channels = decoder.channels();
    let rate = decoder.sample_rate();

    let mut samples = Vec::new();
    for sample in decoder {
        if limit.is_some_and(|max| samples.len() >= max) {
            return Ok(None);
        }
        samples.push(sample);
    }

    Ok(Some(Clip::Memory {
        channels,
        rate,
        samples: samples.into(),
    }))
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

pub struct Pack {
    pub name: String,
    // files that could not be loaded
    pub skipped: Vec<String>,
    clips: Vec<Clip>,
    last: Option<usize>,
}

impl Pack {
    // mode is a built in pack name or custom
    pub fn load(mode: &str, custom_folder: Option<&str>) -> anyhow::Result<Self> {
        if BUILT_IN.contains(&mode) {
            return Self::embedded(mode);
        }
        anyhow::ensure!(mode == "custom", "Unknown mode '{}'", mode);
        let folder = custom_folder.context("No custom folder set")?;
        Self::folder(folder)
    }

    fn embedded(mode: &str) -> anyhow::Result<Self> {
        let dir = AUDIO_DIR
            .get_dir(mode)
            .ok_or_else(|| anyhow::anyhow!("Built-in pack '{}' not found", mode))?;

        let mut clips = Vec::new();
        for file in dir.files().filter(|file| is_audio(file.path())) {
            if let Some(clip) = preload(Cursor::new(file.contents()), None)? {
                clips.push(clip);
            }
        }

        anyhow::ensure!(
            !clips.is_empty(),
            "No embedded audio files found in built-in pack '{}'.",
            mode
        );
        Ok(Self::new(mode, clips))
    }

    fn folder(dir: &str) -> anyhow::Result<Self> {
        let mut clips = Vec::new();
        let mut skipped = Vec::new();

        for entry in
            std::fs::read_dir(dir).with_context(|| format!("Cannot open folder {}", dir))?
        {
            let path = entry?.path();
            if !is_audio(&path) {
                continue;
            }

            let reader = match File::open(&path) {
                Ok(file) => BufReader::new(file),
                Err(err) => {
                    skipped.push(format!("Skipping {}: {}", path.display(), err));
                    continue;
                }
            };

            // the limit is in samples so allow for stereo at 48 khz
            match preload(reader, Some(MAX_PRELOAD_SECS * 48_000 * 2)) {
                Ok(Some(clip)) => clips.push(clip),
                Ok(None) => clips.push(Clip::File(path)),
                Err(err) => skipped.push(format!("Skipping {}: {}", path.display(), err)),
            }
        }

        anyhow::ensure!(
            !clips.is_empty(),
            "No playable audio files found in {}",
            dir
        );
        let mut pack = Self::new("custom", clips);
        pack.skipped = skipped;
        Ok(pack)
    }

    fn new(name: &str, clips: Vec<Clip>) -> Self {
        Self {
            name: name.to_string(),
            skipped: Vec::new(),
            clips,
            last: None,
        }
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    // random pick that never repeats the previous clip
    fn pick(&mut self, rng: &mut ThreadRng) -> &Clip {
        let n = self.clips.len();
        let index = match self.last {
            Some(last) if n > 1 => {
                let i = rng.gen_range(0..n - 1);
                if i >= last { i + 1 } else { i }
            }
            _ => rng.gen_range(0..n),
        };
        self.last = Some(index);
        &self.clips[index]
    }
}

pub struct Player {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    rng: ThreadRng,
}

impl Player {
    pub fn new() -> anyhow::Result<Self> {
        let (stream, handle) =
            OutputStream::try_default().context("No audio output device available")?;

        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            rng: rand::thread_rng(),
        })
    }

    pub fn play(&mut self, pack: &mut Pack, volume: f32, speed: f32) -> anyhow::Result<()> {
        let source = pack.pick(&mut self.rng).source()?;
        let sink = Sink::try_new(&self.handle)?;
        sink.set_volume(volume);
        sink.append(source.speed(speed));
        // replacing the old sink stops it so sounds never pile up
        self.sink = Some(sink);
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        self.sink.as_ref().is_some_and(|sink| !sink.empty())
    }
}

// plays shared samples without copying them for every hit
struct MemorySource {
    samples: Arc<[i16]>,
    pos: usize,
    channels: u16,
    rate: u32,
}

impl Iterator for MemorySource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let sample = self.samples.get(self.pos).copied();
        self.pos += 1;
        sample
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.samples.len().saturating_sub(self.pos);
        (left, Some(left))
    }
}

impl Source for MemorySource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.samples.len().saturating_sub(self.pos))
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        let frames = self.samples.len() as f64 / self.channels.max(1) as f64;
        Some(Duration::from_secs_f64(frames / self.rate.max(1) as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_packs_load() {
        for name in BUILT_IN {
            let pack = Pack::load(name, None).unwrap();
            assert!(pack.len() > 1, "{} pack is empty", name);
        }
    }

    #[test]
    fn pick_never_repeats() {
        let mut pack = Pack::load("pain", None).unwrap();
        let mut rng = rand::thread_rng();
        let mut prev = None;
        for _ in 0..200 {
            pack.pick(&mut rng);
            assert_ne!(pack.last, prev);
            prev = pack.last;
        }
    }
}
