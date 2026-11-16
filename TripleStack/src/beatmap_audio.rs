use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct SoundEvent {
    pub time_ms: f64,
    pub sample: String,
    pub fallback: String,
    pub custom: bool,
    pub map_sample: bool,
    pub gain: f32,
    pub kind: &'static str,
}

#[derive(Clone, Debug)]
pub struct BeatmapAudio {
    pub path: PathBuf,
    pub title: String,
    pub song: Option<PathBuf>,
    pub preview_ms: f64,
    pub duration_ms: f64,
    pub events: Vec<SoundEvent>,
    pub objects: usize,
}

#[derive(Clone, Copy)]
struct Timing {
    time: f64,
    beat: f64,
    set: u32,
    index: u32,
    volume: u32,
    red: bool,
}
#[derive(Clone, Copy)]
struct Control {
    beat: f64,
    sv: f64,
    set: u32,
    index: u32,
    volume: u32,
}
#[derive(Default, Clone)]
struct Sample {
    normal: u32,
    addition: u32,
    index: u32,
    volume: u32,
    filename: String,
}
impl Sample {
    fn parse(text: &str) -> Self {
        let fields: Vec<_> = text.splitn(5, ':').collect();
        let n = |i: usize| {
            fields
                .get(i)
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0)
        };
        Self {
            normal: n(0),
            addition: n(1),
            index: n(2),
            volume: n(3),
            filename: fields.get(4).unwrap_or(&"").to_string(),
        }
    }
}

pub fn audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| ["wav", "ogg", "mp3", "flac"].contains(&x.to_ascii_lowercase().as_str()))
}

pub fn relative_asset(root: &Path, filename: &str) -> Option<PathBuf> {
    let normalized = filename.trim_matches('"').replace('\\', "/");
    let path = Path::new(&normalized);
    (!normalized.is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)))
    .then(|| root.join(path))
}

impl BeatmapAudio {
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(
            path,
            &fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?,
        )
    }

    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        ensure!(
            text.trim_start_matches('\u{feff}')
                .starts_with("osu file format"),
            "Not an osu! beatmap"
        );
        let mut section = "";
        let mut song_name = String::new();
        let mut title = String::new();
        let mut version = String::new();
        let mut default_set = 1;
        let mut mode = 0;
        let mut multiplier = 1.4;
        let mut tick_rate = 1.0;
        let mut preview_ms: f64 = 0.0;
        let mut timings = Vec::new();
        let mut objects = Vec::new();
        let mut extras = Vec::new();
        for (line_index, raw) in text.lines().enumerate() {
            let line = raw.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line;
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim();
                match (section, key.trim()) {
                    ("[General]", "AudioFilename") => song_name = value.to_owned(),
                    ("[General]", "SampleSet") => {
                        default_set = match value {
                            "Soft" => 2,
                            "Drum" => 3,
                            _ => 1,
                        }
                    }
                    ("[General]", "Mode") => mode = value.parse::<u32>()?,
                    ("[General]", "PreviewTime") => preview_ms = value.parse::<f64>()?,
                    ("[Metadata]", "Title") => title = value.to_owned(),
                    ("[Metadata]", "Version") => version = value.to_owned(),
                    ("[Difficulty]", "SliderMultiplier") => multiplier = value.parse::<f64>()?,
                    ("[Difficulty]", "SliderTickRate") => tick_rate = value.parse::<f64>()?,
                    _ => {}
                }
            }
            if section == "[TimingPoints]" {
                let f: Vec<_> = line.split(',').map(str::trim).collect();
                ensure!(
                    f.len() >= 7,
                    "Invalid timing point on line {}",
                    line_index + 1
                );
                let timing = Timing {
                    time: f[0].parse()?,
                    beat: f[1].parse()?,
                    set: f[3].parse()?,
                    index: f[4].parse()?,
                    volume: f[5].parse::<i32>()?.clamp(0, 100) as u32,
                    red: f[6].trim() == "1",
                };
                ensure!(
                    timing.time.is_finite() && timing.beat.is_finite() && timing.beat != 0.0,
                    "Invalid timing value"
                );
                timings.push(timing);
            } else if section == "[HitObjects]" {
                objects.push((line_index + 1, line));
            } else if section == "[Events]"
                && (line.starts_with("Sample,") || line.starts_with("5,"))
            {
                extras.push(line);
            }
        }
        ensure!(
            mode == 0,
            "Beatmap audio timeline currently supports osu!standard (Mode 0)"
        );
        ensure!(
            multiplier.is_finite() && multiplier > 0.0 && tick_rate.is_finite() && tick_rate > 0.0,
            "Invalid slider difficulty values"
        );
        timings.sort_by(|a, b| a.time.total_cmp(&b.time));
        let control_at = |time: f64| {
            let mut c = Control {
                beat: 500.0,
                sv: 1.0,
                set: default_set,
                index: 1,
                volume: 100,
            };
            for t in timings.iter().take_while(|t| t.time <= time) {
                if t.red && t.beat > 0.0 {
                    c.beat = t.beat;
                    c.sv = 1.0;
                } else if !t.red && t.beat < 0.0 {
                    c.sv = (-100.0 / t.beat).clamp(0.1, 10.0);
                }
                c.set = if t.set == 0 { default_set } else { t.set };
                c.index = t.index;
                c.volume = t.volume.min(100);
            }
            c
        };
        let mut events = Vec::new();
        let mut duration_ms: f64 = 0.0;
        for (line, raw) in &objects {
            let f: Vec<_> = raw.split(',').map(str::trim).collect();
            ensure!(f.len() >= 5, "Invalid hit object on line {line}");
            let start: f64 = f[2]
                .parse()
                .with_context(|| format!("Hit object on line {line}"))?;
            ensure!(start.is_finite(), "Invalid object timestamp on line {line}");
            let flags: u32 = f[3].parse()?;
            let sound: u32 = f[4].parse()?;
            duration_ms = duration_ms.max(start);
            if flags & 2 != 0 {
                ensure!(f.len() >= 8, "Incomplete slider on line {line}");
                let repeats: u32 = f[6].parse()?;
                let length: f64 = f[7].parse()?;
                ensure!(
                    (1..=10000).contains(&repeats) && length.is_finite() && length >= 0.0,
                    "Invalid slider on line {line}"
                );
                let sample = Sample::parse(f.get(10).unwrap_or(&""));
                let c = control_at(start);
                let span = length / (multiplier * 100.0 * c.sv) * c.beat;
                ensure!(span.is_finite(), "Invalid slider duration on line {line}");
                duration_ms = duration_ms.max(start + span * f64::from(repeats));
                let edges: Vec<_> = f.get(8).unwrap_or(&"").split('|').collect();
                let sets: Vec<_> = f.get(9).unwrap_or(&"").split('|').collect();
                for edge in 0..=repeats {
                    let time = start + span * f64::from(edge);
                    let mut edge_sample = sample.clone();
                    if let Some(set) = sets.get(edge as usize) {
                        let parsed = Sample::parse(set);
                        edge_sample.normal = parsed.normal;
                        edge_sample.addition = parsed.addition;
                    }
                    let bits = edges
                        .get(edge as usize)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(if edge == 0 { sound } else { 0 });
                    emit(
                        &mut events,
                        time,
                        bits,
                        &edge_sample,
                        control_at(time),
                        "Slider edge",
                    );
                }
                let tick_step = c.beat / tick_rate;
                ensure!(
                    span / tick_step * f64::from(repeats) < 200000.0,
                    "Slider has too many ticks"
                );
                for repeat in 0..repeats {
                    let mut offset = tick_step;
                    while offset < span - 10.0 {
                        let time = start + span * f64::from(repeat) + offset;
                        let c = control_at(time);
                        let set = if sample.normal == 0 {
                            c.set
                        } else {
                            sample.normal
                        };
                        let index = if sample.index == 0 {
                            c.index
                        } else {
                            sample.index
                        };
                        let base = format!("{}-slidertick", set_name(set));
                        events.push(SoundEvent {
                            time_ms: time,
                            sample: indexed(&base, index),
                            fallback: base,
                            custom: false,
                            map_sample: index != 0,
                            gain: gain(&sample, c),
                            kind: "Slider tick",
                        });
                        offset += tick_step;
                    }
                }
            } else if flags & 8 != 0 {
                let end = f.get(5).context("Spinner end is missing")?.parse::<f64>()?;
                ensure!(end.is_finite(), "Invalid spinner end on line {line}");
                // Some legacy maps use backwards spinner ranges. Treat them as
                // zero-duration spinners rather than rejecting the whole map.
                let end = end.max(start);
                duration_ms = duration_ms.max(end);
                emit(
                    &mut events,
                    end,
                    sound,
                    &Sample::parse(f.get(6).unwrap_or(&"")),
                    control_at(end),
                    "Spinner end",
                );
            } else if flags & 1 != 0 {
                emit(
                    &mut events,
                    start,
                    sound,
                    &Sample::parse(f.get(5).unwrap_or(&"")),
                    control_at(start),
                    "Circle",
                );
            } else {
                bail!("Unsupported hit object on line {line}");
            }
            ensure!(events.len() < 500000, "Beatmap has too many audio events");
        }
        for raw in extras {
            let f: Vec<_> = raw.splitn(5, ',').collect();
            if f.len() == 5 {
                let time = f[1].parse::<f64>()?;
                ensure!(time.is_finite(), "Invalid storyboard sample timestamp");
                duration_ms = duration_ms.max(time);
                events.push(SoundEvent {
                    time_ms: time,
                    sample: f[3].trim_matches('"').to_owned(),
                    fallback: String::new(),
                    custom: true,
                    map_sample: true,
                    gain: f[4].parse::<f32>()?.clamp(0.0, 100.0) / 100.0,
                    kind: "Custom sample",
                });
            }
        }
        events.sort_by(|a, b| a.time_ms.total_cmp(&b.time_ms));
        let root = path.parent().unwrap_or(Path::new("."));
        let song = relative_asset(root, &song_name);
        Ok(Self {
            path: path.to_owned(),
            title: format!("{title} [{version}]"),
            song,
            preview_ms: preview_ms.max(0.0),
            duration_ms: duration_ms + 1500.0,
            events,
            objects: objects.len(),
        })
    }
}
fn set_name(set: u32) -> &'static str {
    match set {
        2 => "soft",
        3 => "drum",
        _ => "normal",
    }
}
fn indexed(base: &str, index: u32) -> String {
    if index > 1 {
        format!("{base}{index}")
    } else {
        base.to_owned()
    }
}
fn gain(sample: &Sample, c: Control) -> f32 {
    (if sample.volume == 0 {
        c.volume
    } else {
        sample.volume.min(100)
    }) as f32
        / 100.0
}
fn emit(
    out: &mut Vec<SoundEvent>,
    time: f64,
    bits: u32,
    sample: &Sample,
    c: Control,
    kind: &'static str,
) {
    if !sample.filename.is_empty() {
        out.push(SoundEvent {
            time_ms: time,
            sample: sample.filename.clone(),
            fallback: String::new(),
            custom: true,
            map_sample: true,
            gain: gain(sample, c),
            kind,
        });
        return;
    }
    let normal = if sample.normal == 0 {
        c.set
    } else {
        sample.normal
    };
    let addition = if sample.addition == 0 {
        normal
    } else {
        sample.addition
    };
    let index = if sample.index == 0 {
        c.index
    } else {
        sample.index
    };
    for (bit, name) in [(0, "normal"), (2, "whistle"), (4, "finish"), (8, "clap")] {
        if bit == 0 || bits & bit != 0 {
            let base = format!(
                "{}-hit{name}",
                set_name(if bit == 0 { normal } else { addition })
            );
            out.push(SoundEvent {
                time_ms: time,
                sample: indexed(&base, index),
                fallback: base,
                custom: false,
                map_sample: index != 0,
                gain: gain(sample, c),
                kind,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timing_inheritance_numbered_samples_layers_and_slider_edges() {
        let text = "osu file format v14\n[General]\nAudioFilename: song.mp3\nSampleSet: Normal\n[Difficulty]\nSliderMultiplier: 1\nSliderTickRate: 1\n[TimingPoints]\n0,500,4,2,12,70,1,0\n1000,-50,4,2,2,30,0,0\n[HitObjects]\n0,0,500,1,10,0:0:0:0:\n0,0,1000,2,0,L|100:0,2,200,0|8|4,0:0|1:2|2:1,0:0:0:0:\n0,0,2500,8,4,3000,0:0:0:80:\n0,0,3500,1,8,0:0:0:0:custom.wav\n";
        let map = BeatmapAudio::parse(Path::new("test.osu"), text).unwrap();
        assert_eq!(map.events[0].sample, "soft-hitnormal12");
        assert_eq!(map.events[1].sample, "soft-hitwhistle12");
        assert_eq!(map.events[2].sample, "soft-hitclap12");
        assert_eq!(map.events[0].gain, 0.7);
        assert!(
            map.events
                .iter()
                .any(|e| e.time_ms == 1500.0 && e.sample == "soft-hitclap2" && e.gain == 0.3)
        );
        assert!(
            map.events
                .iter()
                .any(|e| e.time_ms == 2000.0 && e.sample == "normal-hitfinish2")
        );
        assert!(
            map.events
                .iter()
                .any(|e| e.time_ms == 3000.0 && e.kind == "Spinner end" && e.gain == 0.8)
        );
        assert_eq!(map.events.iter().filter(|e| e.time_ms == 3500.0).count(), 1);
    }
    #[test]
    fn rejects_invalid_maps_and_escaping_paths() {
        assert!(BeatmapAudio::parse(Path::new("bad.osu"), "not a map").is_err());
        assert!(relative_asset(Path::new("test"), "../private.wav").is_none());
        assert!(relative_asset(Path::new("test"), "samples/hit.wav").is_some());
    }
    #[test]
    fn legacy_out_of_range_timing_volumes_are_clamped() {
        let text = "osu file format v14\n[TimingPoints]\n0,500,4,1,1,-100,1,0\n1000,-100,4,1,1,150,0,0\n[HitObjects]\n0,0,500,1,0,0:0:0:0:\n0,0,1500,1,0,0:0:0:0:\n";
        let map = BeatmapAudio::parse(Path::new("legacy.osu"), text).unwrap();
        assert_eq!(map.events[0].gain, 0.0);
        assert_eq!(map.events[1].gain, 1.0);
    }
    #[test]
    fn backwards_legacy_spinner_uses_start_as_end() {
        let map = BeatmapAudio::parse(
            Path::new("legacy.osu"),
            "osu file format v14\n[HitObjects]\n256,192,36669,12,0,36295,0:0:0:0:\n",
        )
        .unwrap();
        assert_eq!(map.events[0].time_ms, 36669.0);
        assert_eq!(map.events[0].kind, "Spinner end");
    }
}
