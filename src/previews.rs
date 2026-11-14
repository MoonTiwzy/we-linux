//! Preview images: fetched (or read from the disk), decoded and scaled on a
//! few worker threads, kept in a cache directory. A job that is no longer
//! wanted (the page changed) is dropped before it starts.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

pub enum Source {
    /// A Steam preview URL: asked for at the job's size.
    Url(String),
    File(PathBuf),
}

pub struct Job {
    /// What the result is for: an item id.
    pub id: u64,
    pub source: Source,
    /// Longest side in pixels.
    pub size: u32,
    /// Dropped unless this is still [`Previews::generation`] (0: never dropped).
    pub generation: u64,
    /// The side panel's picture rather than a grid cell's.
    pub large: bool,
    /// Every frame of an animated preview, not the first only.
    pub animate: bool,
}

pub struct Decoded {
    pub id: u64,
    pub generation: u64,
    pub large: bool,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// How long the first frame of an animated preview shows, in milliseconds.
    pub delay: u32,
    /// The rest of an animated preview (jobs that animate): frames the size
    /// of the first, each with how long it shows in milliseconds.
    pub frames: Vec<(Vec<u8>, u32)>,
}

pub struct Previews {
    jobs: Sender<Job>,
    pub generation: Arc<AtomicU64>,
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"))
        .join("we-linux/previews")
}

/// A workshop preview as Steam has it (a 192-512 px GIF or JPEG; its image
/// server only pads on request, it does not scale up), cached by item id and
/// by its URL: an item whose preview was changed has another.
fn fetch(url: &str, id: u64) -> Result<Vec<u8>, String> {
    let cached = cache_dir().join(format!("{id}-{:016x}", crate::engine::fnv(url)));
    if let Ok(bytes) = std::fs::read(&cached)
        && !bytes.is_empty()
    {
        return Ok(bytes);
    }
    let bytes = ureq::get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(32 << 20)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    // Written whole: a grid cell and the side panel may fetch the same
    // preview at once, and neither reads half of the other's file.
    static PART: AtomicU64 = AtomicU64::new(0);
    let part = cache_dir().join(format!("{id}.{}.{}.part", std::process::id(), PART.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::create_dir_all(cache_dir());
    if std::fs::write(&part, &bytes).and_then(|()| std::fs::rename(&part, &cached)).is_err() {
        let _ = std::fs::remove_file(&part);
        return Ok(bytes);
    }
    // The item's earlier previews (and the ones kept by id alone before).
    let earlier = format!("{id}-");
    if let Ok(files) = std::fs::read_dir(cache_dir()) {
        for file in files.flatten().map(|f| f.path()) {
            let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let old = name == id.to_string() || (name.starts_with(&earlier) && !name.ends_with(".part"));
            if old && file != cached {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
    Ok(bytes)
}

/// What the frames of the side panel's animated preview may take, decoded.
const ANIMATION_BYTES: usize = 24 << 20;
/// What the frames of a grid cell's may take: a screenful of cells plays.
const CELL_ANIMATION_BYTES: usize = 4 << 20;

/// The frames of a GIF (up to 150 are read), all at the size of the first and
/// no larger than `size`. A long one loses frames evenly (their time goes to
/// the ones kept) until it fits `budget`.
fn animation(bytes: &[u8], size: u32, budget: usize) -> Option<Vec<(image::RgbaImage, u32)>> {
    use image::AnimationDecoder;
    if !bytes.starts_with(b"GIF8") {
        return None;
    }
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let mut out = Vec::new();
    for frame in decoder.into_frames().take(150) {
        let frame = frame.ok()?;
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = (numerator / denominator.max(1)).clamp(20, 1000);
        let frame = frame.into_buffer();
        let frame = if frame.width().max(frame.height()) > size {
            let scale = f64::from(size) / f64::from(frame.width().max(frame.height()));
            let (w, h) = ((f64::from(frame.width()) * scale) as u32, (f64::from(frame.height()) * scale) as u32);
            image::imageops::thumbnail(&frame, w.max(1), h.max(1))
        } else {
            frame
        };
        out.push((frame, delay));
    }
    let bytes: usize = out.iter().map(|(f, _)| f.as_raw().len()).sum();
    let step = bytes.div_ceil(budget).max(1);
    if step > 1 {
        out = out
            .chunks(step)
            .map(|group| (group[0].0.clone(), group.iter().map(|(_, d)| d).sum::<u32>().min(1000)))
            .collect();
    }
    (out.len() > 1).then_some(out)
}

fn decode(job: &Job) -> Result<Decoded, String> {
    let bytes = match &job.source {
        Source::Url(url) => fetch(url, job.id)?,
        Source::File(path) => std::fs::read(path).map_err(|e| e.to_string())?,
    };
    let budget = if job.large { ANIMATION_BYTES } else { CELL_ANIMATION_BYTES };
    if job.animate
        && let Some(mut frames) = animation(&bytes, job.size, budget)
    {
        let (first, delay) = frames.remove(0);
        let (width, height) = (first.width(), first.height());
        return Ok(Decoded {
            id: job.id,
            generation: job.generation,
            large: job.large,
            width,
            height,
            rgba: first.into_raw(),
            delay,
            frames: frames
                .into_iter()
                .filter(|(f, _)| f.width() == width && f.height() == height)
                .map(|(f, delay)| (f.into_raw(), delay))
                .collect(),
        });
    }
    let image = image::load_from_memory(&bytes).map_err(|e| e.to_string())?;
    let image = if image.width().max(image.height()) > job.size {
        image.thumbnail(job.size, job.size)
    } else {
        image
    };
    let rgba = image.to_rgba8();
    Ok(Decoded {
        id: job.id,
        generation: job.generation,
        large: job.large,
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
        delay: 0,
        frames: Vec::new(),
    })
}

impl Previews {
    pub fn new(done: impl Fn(Decoded) + Send + Sync + 'static) -> Self {
        let (jobs, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let generation = Arc::new(AtomicU64::new(1));
        let done = Arc::new(done);
        for n in 0..12 {
            let (rx, generation, done) = (rx.clone(), generation.clone(), done.clone());
            std::thread::Builder::new()
                .name(format!("preview-{n}"))
                .spawn(move || {
                    loop {
                        let job = match rx.lock().unwrap_or_else(|e| e.into_inner()).recv() {
                            Ok(job) => job,
                            Err(_) => return,
                        };
                        if job.generation != 0 && job.generation != generation.load(Ordering::Relaxed) {
                            continue;
                        }
                        if let Ok(decoded) = decode(&job) {
                            done(decoded);
                        }
                    }
                })
                .expect("preview thread");
        }
        Self { jobs, generation }
    }

    pub fn request(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Jobs queued before this are dropped; returns the generation new ones carry.
    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }
}
