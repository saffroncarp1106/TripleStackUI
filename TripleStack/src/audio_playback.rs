use anyhow::{Context, Result};
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, Source, buffer::SamplesBuffer};
use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct ScheduledSound {
    pub time_ms: f64,
    pub path: PathBuf,
    pub gain: f32,
}
pub struct PlaybackPlan {
    pub song: Option<PathBuf>,
    pub events: Vec<ScheduledSound>,
    pub from_ms: f64,
    pub end_ms: f64,
    pub music_volume: f32,
    pub hit_volume: f32,
}
#[derive(Default, Clone)]
pub struct PlaybackStatus {
    pub playing: bool,
    pub paused: bool,
    pub position_ms: f64,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}
enum Command {
    Preview(PathBuf, f32),
    Play(PlaybackPlan),
    ReplaceEvents(Vec<ScheduledSound>),
    Pause(bool),
    Volumes(f32, f32),
    Stop,
    Shutdown,
    OutputFailed(u64, String),
}
pub struct AudioBackend {
    tx: mpsc::Sender<Command>,
    state: Arc<Mutex<PlaybackStatus>>,
    repaint_context: Arc<Mutex<Option<eframe::egui::Context>>>,
}
impl Default for AudioBackend {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(PlaybackStatus::default()));
        let shared = state.clone();
        let repaint_context = Arc::new(Mutex::new(None));
        let worker_context = repaint_context.clone();
        let worker_tx = tx.clone();
        std::thread::spawn(move || worker(rx, worker_tx, shared, worker_context));
        Self {
            tx,
            state,
            repaint_context,
        }
    }
}
impl Drop for AudioBackend {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}
impl AudioBackend {
    pub fn attach_context(&self, ctx: &eframe::egui::Context) {
        *self.repaint_context.lock().unwrap() = Some(ctx.clone());
    }
    pub fn status(&self) -> PlaybackStatus {
        self.state.lock().unwrap().clone()
    }
    pub fn preview(&self, path: PathBuf, volume: f32) {
        let _ = self.tx.send(Command::Preview(path, volume));
    }
    pub fn play(&self, plan: PlaybackPlan) {
        let _ = self.tx.send(Command::Play(plan));
    }
    pub fn replace_events(&self, events: Vec<ScheduledSound>) {
        let _ = self.tx.send(Command::ReplaceEvents(events));
    }
    pub fn pause(&self, value: bool) {
        let _ = self.tx.send(Command::Pause(value));
    }
    pub fn volumes(&self, music: f32, hits: f32) {
        let _ = self.tx.send(Command::Volumes(music, hits));
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }
}
struct Track {
    song_path: Option<PathBuf>,
    music_volume: f32,
    clock_advanced: Instant,
    song: Option<Sink>,
    events: Vec<ScheduledSound>,
    samples: HashMap<PathBuf, SamplesBuffer>,
    next: usize,
    base_ms: f64,
    epoch: Instant,
    paused: bool,
    end_ms: f64,
    hit_volume: f32,
    last_position_ms: f64,
    voices: Vec<(Sink, f32)>,
    warnings: Vec<String>,
}
impl Track {
    fn position(&self) -> f64 {
        if self.paused {
            return self.last_position_ms;
        }
        self.base_ms
            + if let Some(song) = &self.song {
                song.get_pos().as_secs_f64() * 1000.0
            } else if self.paused {
                0.0
            } else {
                self.epoch.elapsed().as_secs_f64() * 1000.0
            }
    }
    fn new(plan: PlaybackPlan, stream: &OutputStream) -> Result<Self> {
        let song_path = plan.song.clone();
        let (samples, warnings) = cache_samples(&plan.events);
        let song = if let Some(path) = plan.song {
            let decoder = decode(&path)?;
            let sink = Sink::connect_new(stream.mixer());
            sink.set_volume(plan.music_volume);
            sink.append(
                decoder.skip_duration(Duration::from_secs_f64(plan.from_ms.max(0.0) / 1000.0)),
            );
            Some(sink)
        } else {
            None
        };
        let next = plan
            .events
            .partition_point(|event| event.time_ms < plan.from_ms);
        Ok(Self {
            song_path,
            music_volume: plan.music_volume,
            clock_advanced: Instant::now(),
            song,
            events: plan.events,
            samples,
            next,
            base_ms: plan.from_ms,
            epoch: Instant::now(),
            paused: false,
            end_ms: plan.end_ms,
            hit_volume: plan.hit_volume,
            last_position_ms: plan.from_ms,
            voices: Vec::new(),
            warnings,
        })
    }
    fn replace_events(&mut self, events: Vec<ScheduledSound>) {
        let (samples, warnings) = cache_samples(&events);
        let time = self.position();
        self.next = events.partition_point(|event| event.time_ms < time);
        self.events = events;
        self.samples = samples;
        self.warnings = warnings;
        // Do not overlap the old hitsound tails with the new audition.
        self.voices.clear();
    }
    fn pause(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.last_position_ms = self.position();
        if self.song.is_none() {
            self.base_ms = self.last_position_ms;
            self.epoch = Instant::now();
        }
        self.paused = paused;
        self.clock_advanced = Instant::now();
        if let Some(song) = &self.song {
            if paused {
                song.pause();
            } else {
                song.play();
            }
        }
        for (sink, _) in &self.voices {
            if paused {
                sink.pause();
            } else {
                sink.play();
            }
        }
    }
    fn tick(&mut self, stream: &OutputStream) {
        self.voices.retain(|(sink, _)| !sink.empty());
        if self.paused {
            return;
        }
        // Continue the timeline if the music ends before the final map event.
        if self.song.as_ref().is_some_and(Sink::empty) {
            self.song = None;
            self.base_ms = self.last_position_ms;
            self.epoch = Instant::now();
        }
        let time = self.position();
        if time > self.last_position_ms + 0.1 {
            self.clock_advanced = Instant::now();
        }
        self.last_position_ms = time;
        while let Some(event) = self
            .events
            .get(self.next)
            .filter(|event| event.time_ms <= time)
        {
            if let Some(sample) = self.samples.get(&event.path) {
                let sink = Sink::connect_new(stream.mixer());
                sink.set_volume(event.gain * self.hit_volume);
                sink.append(sample.clone());
                self.voices.push((sink, event.gain));
            }
            self.next += 1;
        }
    }
}
impl Track {
    fn recovery_plan(&self) -> PlaybackPlan {
        PlaybackPlan {
            song: self.song_path.clone(),
            events: self.events.clone(),
            from_ms: self.last_position_ms,
            end_ms: self.end_ms,
            music_volume: self.music_volume,
            hit_volume: self.hit_volume,
        }
    }
    fn clock_stalled(&self) -> bool {
        !self.paused
            && self.song.is_some()
            && self.clock_advanced.elapsed() >= Duration::from_secs(2)
    }
}
fn cache_samples(events: &[ScheduledSound]) -> (HashMap<PathBuf, SamplesBuffer>, Vec<String>) {
    let mut samples = HashMap::new();
    let mut warnings = Vec::new();
    let mut attempted = std::collections::HashSet::new();
    for event in events {
        if attempted.insert(event.path.clone()) {
            let (sample, warning) = load_sample(&event.path);
            if let Some(sample) = sample {
                samples.insert(event.path.clone(), sample);
            }
            if let Some(warning) = warning {
                warnings.push(warning);
            }
        }
    }
    (samples, warnings)
}

fn decoded_sample(path: &Path) -> Result<SamplesBuffer> {
    // Empty files and zero-data WAVs are commonly used to intentionally mute a sample.
    let bytes = std::fs::read(path).with_context(|| format!("Reading {}", path.display()))?;
    let mut silent = bytes.is_empty();
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        let mut offset = 12usize;
        while offset + 8 <= bytes.len() {
            let size =
                u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
            if bytes.get(offset..offset + 4) == Some(b"data") {
                silent = size == 0;
                break;
            }
            let Some(next) = offset.checked_add(8 + size + size % 2) else {
                break;
            };
            offset = next;
        }
    }
    if silent {
        return Ok(SamplesBuffer::new(1, 44100, vec![0.0]));
    }
    let decoder = decode(path)?;
    let (channels, rate) = (decoder.channels(), decoder.sample_rate());
    Ok(SamplesBuffer::new(
        channels,
        rate,
        decoder.collect::<Vec<f32>>(),
    ))
}

pub(crate) fn load_sample(path: &Path) -> (Option<SamplesBuffer>, Option<String>) {
    match decoded_sample(path) {
        Ok(sample) => (Some(sample), None),
        Err(error) => (None, Some(format!("Ignored unreadable sample: {error:#}"))),
    }
}

fn decode(path: &Path) -> Result<Decoder<std::io::BufReader<File>>> {
    Decoder::try_from(File::open(path).with_context(|| format!("Opening {}", path.display()))?)
        .with_context(|| format!("Decoding {}", path.display()))
}
fn default_output_key() -> Option<String> {
    let device = rodio::cpal::default_host().default_output_device()?;
    let config = device.default_output_config().ok()?;
    Some(format!(
        "{}:{}:{}:{:?}",
        device.name().ok()?,
        config.channels(),
        config.sample_rate().0,
        config.sample_format()
    ))
}
fn open_output(tx: &mpsc::Sender<Command>, generation: &mut u64) -> Result<OutputStream> {
    *generation += 1;
    let current = *generation;
    let tx = tx.clone();
    let mut stream = OutputStreamBuilder::from_default_device()
        .context("Finding the default audio output")?
        .with_error_callback(move |error| {
            let _ = tx.send(Command::OutputFailed(current, error.to_string()));
        })
        .open_stream_or_fallback()
        .context("Opening the default audio output")?;
    stream.log_on_drop(false);
    Ok(stream)
}
fn worker(
    rx: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Command>,
    state: Arc<Mutex<PlaybackStatus>>,
    repaint_context: Arc<Mutex<Option<eframe::egui::Context>>>,
) {
    let mut stream: Option<OutputStream> = None;
    let mut track: Option<Track> = None;
    let mut preview: Option<Sink> = None;
    let mut generation = 0;
    let mut output_key = None;
    let mut checked_output = Instant::now();
    loop {
        let received = if track.as_ref().is_some_and(|track| !track.paused) {
            rx.recv_timeout(Duration::from_millis(4))
        } else if track.is_some() || preview.is_some() {
            rx.recv_timeout(Duration::from_millis(500))
        } else {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let mut notify_ui = received.is_ok();
        let mut output_failure = None;
        match received {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(command) => {
                let result = (|| -> Result<()> {
                    if matches!(command, Command::Play(_))
                        || (matches!(command, Command::Preview(_, _)) && track.is_none())
                    {
                        track = None;
                        preview = None;
                        stream = None;
                        stream = Some(open_output(&tx, &mut generation)?);
                        output_key = default_output_key();
                        checked_output = Instant::now();
                    }
                    match command {
                        Command::Preview(path, gain) => {
                            drop(preview.take());
                            let sink = Sink::connect_new(stream.as_ref().unwrap().mixer());
                            sink.set_volume(gain);
                            let warning = match decode(&path) {
                                Ok(source) => {
                                    sink.append(source);
                                    None
                                }
                                Err(_) => {
                                    let (sample, warning) = load_sample(&path);
                                    if let Some(sample) = sample {
                                        sink.append(sample);
                                    }
                                    warning
                                }
                            };
                            let mut status = state.lock().unwrap();
                            status.error = None;
                            status.warnings = warning.into_iter().collect();
                            preview = Some(sink);
                        }
                        Command::Play(plan) => {
                            track = None;
                            preview = None;
                            track = Some(Track::new(plan, stream.as_ref().unwrap())?);
                            let mut status = state.lock().unwrap();
                            status.error = None;
                            status.warnings = track.as_ref().unwrap().warnings.clone();
                        }
                        Command::ReplaceEvents(events) => {
                            if let Some(track) = &mut track {
                                track.replace_events(events);
                                state.lock().unwrap().warnings = track.warnings.clone();
                            }
                        }
                        Command::Pause(paused) => {
                            if let Some(track) = &mut track {
                                track.pause(paused);
                            }
                        }
                        Command::Volumes(music, hits) => {
                            if let Some(track) = &mut track {
                                track.music_volume = music;
                                track.hit_volume = hits;
                                if let Some(song) = &track.song {
                                    song.set_volume(music);
                                }
                                for (sink, gain) in &track.voices {
                                    sink.set_volume(*gain * hits);
                                }
                            }
                        }
                        Command::Stop => {
                            track = None;
                            preview = None;
                        }
                        Command::Shutdown => {}
                        Command::OutputFailed(failed_generation, reason) => {
                            if stream.is_some() && failed_generation == generation {
                                output_failure = Some(reason);
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    state.lock().unwrap().error = Some(format!("{error:#}"));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if stream.is_some()
            && (track.is_some() || preview.is_some())
            && checked_output.elapsed() >= Duration::from_millis(500)
        {
            checked_output = Instant::now();
            if default_output_key() != output_key {
                output_failure = Some("The default audio device changed".into());
            }
        }
        if track.as_ref().is_some_and(Track::clock_stalled) {
            output_failure = Some("The audio device stopped advancing playback".into());
        }
        if let Some(reason) = output_failure {
            let recovery = track
                .take()
                .map(|track| (track.recovery_plan(), track.paused));
            preview = None;
            stream = None;
            let result = (|| -> Result<()> {
                if let Some((plan, paused)) = recovery {
                    stream = Some(open_output(&tx, &mut generation)?);
                    output_key = default_output_key();
                    checked_output = Instant::now();
                    let mut resumed = Track::new(plan, stream.as_ref().unwrap())?;
                    resumed.pause(paused);
                    track = Some(resumed);
                }
                Ok(())
            })();
            let mut status = state.lock().unwrap();
            status.error = result.err().map(|error| {
                format!("{reason}. {error:#}. Press Play to reconnect when the device is ready.")
            });
            notify_ui = true;
        }
        if let Some(current) = &mut track {
            current.tick(stream.as_ref().unwrap());
            let time = current.position();
            let mut status = state.lock().unwrap();
            status.position_ms = time;
            status.playing = true;
            status.paused = current.paused;
            if time >= current.end_ms {
                drop(status);
                track = None;
                notify_ui = true;
            }
        }
        if track.is_none() {
            let mut status = state.lock().unwrap();
            status.playing = false;
            status.paused = false;
        }
        if preview.as_ref().is_some_and(Sink::empty) {
            preview = None;
        }
        // Commands, errors and completion wake the UI once. Progress repainting
        // is owned by the visible Audio tab, not by this audio scheduling loop.
        if notify_ui && let Some(ctx) = repaint_context.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_commands_wake_an_idle_ui_without_an_animation_loop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let ctx = eframe::egui::Context::default();
        for _ in 0..4 {
            let _ = ctx.run(Default::default(), |_| {});
        }
        let wakeups = Arc::new(AtomicUsize::new(0));
        let observed = wakeups.clone();
        ctx.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        });
        let backend = AudioBackend::default();
        backend.attach_context(&ctx);
        backend.stop(); // Does not require an audio device.
        let deadline = Instant::now() + Duration::from_secs(1);
        while wakeups.load(Ordering::SeqCst) == 0 {
            assert!(
                Instant::now() < deadline,
                "audio command did not wake the idle UI"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let count = wakeups.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(wakeups.load(Ordering::SeqCst), count);
    }

    #[test]
    fn bad_samples_are_ignored_and_intentional_silence_is_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("normal-slidertick.wav");
        std::fs::write(&path, b"invalid or unsupported audio").unwrap();
        let (sample, warning) = load_sample(&path);
        assert!(sample.is_none());
        assert!(warning.unwrap().contains("Ignored unreadable sample"));
        assert!(
            load_sample(&directory.path().join("missing.wav"))
                .0
                .is_none()
        );
        std::fs::write(&path, []).unwrap();
        let (sample, warning) = load_sample(&path);
        assert_eq!(sample.unwrap().collect::<Vec<_>>(), vec![0.0]);
        assert!(warning.is_none());
        // A PCM WAV with a zero-length data chunk is also intentionally silent.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, wav).unwrap();
        let (sample, warning) = load_sample(&path);
        assert_eq!(sample.unwrap().collect::<Vec<_>>(), vec![0.0]);
        assert!(warning.is_none());
    }
    #[test]
    #[ignore = "requires a default audio output and TRIPLESTACK_TEST_SKIN / TRIPLESTACK_TEST_BEATMAP"]
    fn muted_output_play_pause_resume_and_stop() {
        let skin = PathBuf::from(std::env::var_os("TRIPLESTACK_TEST_SKIN").unwrap());
        let map = triplestack::beatmap_audio::BeatmapAudio::load(&PathBuf::from(
            std::env::var_os("TRIPLESTACK_TEST_BEATMAP").unwrap(),
        ))
        .unwrap();
        let invalid_directory = tempfile::tempdir().unwrap();
        let invalid = invalid_directory.path().join("broken.wav");
        std::fs::write(&invalid, b"unsupported audio").unwrap();
        let backend = AudioBackend::default();
        backend.play(PlaybackPlan {
            song: map.song,
            events: vec![
                ScheduledSound {
                    time_ms: 0.0,
                    path: skin.join("normal-hitnormal.wav"),
                    gain: 0.0,
                },
                ScheduledSound {
                    time_ms: 1.0,
                    path: invalid,
                    gain: 0.0,
                },
                ScheduledSound {
                    time_ms: 2.0,
                    path: invalid_directory.path().join("missing.wav"),
                    gain: 0.0,
                },
            ],
            from_ms: 0.0,
            end_ms: 10000.0,
            music_volume: 0.0,
            hit_volume: 0.0,
        });
        let wait_for = |predicate: &dyn Fn(&PlaybackStatus) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let status = backend.status();
                assert!(status.error.is_none(), "{:?}", status.error);
                if predicate(&status) {
                    return status;
                }
                assert!(Instant::now() < deadline, "audio state timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let playing = wait_for(&|s| s.playing && s.position_ms > 50.0);
        assert_eq!(playing.warnings.len(), 2);
        backend.pause(true);
        let paused = wait_for(&|s| s.paused);
        std::thread::sleep(Duration::from_millis(80));
        assert!((backend.status().position_ms - paused.position_ms).abs() < 1.0);
        // Switching the audition bank must preserve the song clock and pause state.
        backend.replace_events(vec![ScheduledSound {
            time_ms: 1000.0,
            path: skin.join("normal-hitclap.wav"),
            gain: 0.0,
        }]);
        let switched = wait_for(&|status| status.warnings.is_empty());
        assert!(switched.playing && switched.paused);
        assert_eq!(switched.position_ms, paused.position_ms);
        // Exercise the same worker message sent by the device's error callback.
        backend
            .tx
            .send(Command::OutputFailed(1, "simulated device reset".into()))
            .unwrap();
        backend.replace_events(vec![ScheduledSound {
            time_ms: 1000.0,
            path: invalid_directory.path().join("reset-marker.wav"),
            gain: 0.0,
        }]);
        let recovered = wait_for(&|s| s.warnings.len() == 1);
        assert!(recovered.playing && recovered.paused);
        assert!((recovered.position_ms - paused.position_ms).abs() < 2.0);
        // A delayed error from the discarded device must not reset its successor.
        backend
            .tx
            .send(Command::OutputFailed(1, "stale reset".into()))
            .unwrap();
        backend.replace_events(Vec::new());
        let stable = wait_for(&|s| s.warnings.is_empty());
        assert!(stable.playing && stable.paused);
        backend.pause(false);
        wait_for(&|s| s.playing && !s.paused && s.position_ms > paused.position_ms + 20.0);
        backend
            .tx
            .send(Command::OutputFailed(2, "second device reset".into()))
            .unwrap();
        backend.replace_events(vec![ScheduledSound {
            time_ms: 1000.0,
            path: invalid_directory.path().join("second-reset-marker.wav"),
            gain: 0.0,
        }]);
        let recovered_again = wait_for(&|s| s.warnings.len() == 1);
        assert!(recovered_again.playing && !recovered_again.paused);
        wait_for(&|s| s.position_ms > recovered_again.position_ms + 20.0);
        backend.stop();
        wait_for(&|s| !s.playing);
        // A subsequent Play must initialize an output again even after resets.
        backend.play(PlaybackPlan {
            song: None,
            events: Vec::new(),
            from_ms: 500.0,
            end_ms: 10000.0,
            music_volume: 0.0,
            hit_volume: 0.0,
        });
        wait_for(&|s| s.playing && s.position_ms > 520.0);
        backend.stop();
        wait_for(&|s| !s.playing);
    }
}
