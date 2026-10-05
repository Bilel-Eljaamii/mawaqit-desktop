use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use rodio::mixer::Mixer;
use rodio::{Decoder, DeviceSinkBuilder, Player};

/// Embedded so the adhan plays no matter where the app is launched from.
const ATHAN_MP3: &[u8] = include_bytes!("../../assets/athan.mp3");

/// How often the audio thread notices that the athan finished on its own.
const PLAYBACK_POLL: Duration = Duration::from_millis(250);

/// What to play: the embedded athan, or a user-chosen local audio file. The
/// path comes from the attacker-writable config file — it is opened as audio
/// input only (decode errors are swallowed, never a panic or a code path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AthanSource {
    Builtin,
    File(PathBuf),
}

enum Command {
    Play { source: AthanSource, volume: f32 },
    Stop,
}

static COMMANDS: OnceLock<Sender<Command>> = OnceLock::new();
static PLAYING: AtomicBool = AtomicBool::new(false);

/// The 0–1 rodio volume for a configured 0–100 percent (`None` = full
/// volume). Out-of-range config values clamp instead of misbehaving.
pub fn effective_volume(percent: Option<u8>) -> f32 {
    match percent {
        None => 1.0,
        Some(p) => (f32::from(p) / 100.0).clamp(0.0, 1.0),
    }
}

/// Start playing the athan (non-blocking). Returns false when the audio
/// thread is gone. A playback already running is replaced. The playing flag
/// is set here so [`is_playing`] is true as soon as this returns; the audio
/// thread clears it if the source is unavailable or playback ends/stops.
pub fn play_athan(source: AthanSource, volume_percent: Option<u8>) -> bool {
    let sender = COMMANDS.get_or_init(spawn_audio_thread);
    PLAYING.store(true, Ordering::SeqCst);
    let play = Command::Play { source, volume: effective_volume(volume_percent) };
    if sender.send(play).is_err() {
        PLAYING.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

/// Manually cut off the athan (tray item / UI button / IPC command).
pub fn stop_athan() {
    PLAYING.store(false, Ordering::SeqCst);
    if let Some(sender) = COMMANDS.get() {
        let _ = sender.send(Command::Stop);
    }
}

/// True while an athan is sounding (drives the UI's stop button).
pub fn is_playing() -> bool {
    PLAYING.load(Ordering::SeqCst)
}

/// Dedicated thread owning the audio device, so playback state never has to
/// cross threads; control goes over a channel.
fn spawn_audio_thread() -> Sender<Command> {
    let (tx, rx) = mpsc::channel::<Command>();
    thread::spawn(move || {
        let Ok(device) = DeviceSinkBuilder::open_default_sink() else {
            eprintln!("No audio output device available for athan playback");
            PLAYING.store(false, Ordering::SeqCst);
            return;
        };
        let mixer = device.mixer().clone();

        let mut current: Option<Player> = None;
        loop {
            match rx.recv_timeout(PLAYBACK_POLL) {
                Ok(Command::Play { source, volume }) => {
                    if let Some(sink) = &current {
                        sink.stop();
                    }
                    match play_once(&mixer, &source, volume) {
                        Ok(sink) => current = Some(sink),
                        Err(e) => {
                            eprintln!("{e}");
                            PLAYING.store(false, Ordering::SeqCst);
                            // A cached voice file that fails to decode must
                            // never silence the adhan: drop the corrupt file
                            // and retry once with the builtin recording.
                            if let AthanSource::File(path) = &source {
                                let _ = std::fs::remove_file(path);
                                match play_once(&mixer, &AthanSource::Builtin, volume) {
                                    Ok(sink) => {
                                        current = Some(sink);
                                        PLAYING.store(true, Ordering::SeqCst);
                                    }
                                    Err(builtin_err) => {
                                        eprintln!("builtin athan fallback failed: {builtin_err}");
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(Command::Stop) => {
                    if let Some(sink) = &current {
                        sink.stop();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            // Clear the playing flag when playback ended on its own.
            if let Some(sink) = &current {
                if sink.empty() {
                    current = None;
                    PLAYING.store(false, Ordering::SeqCst);
                }
            }
        }
    });
    tx
}

/// The opened audio input, before decoding. Kept as an enum so the embedded
/// bytes (no filesystem) and a custom file flow through the same decoder.
enum MediaSource {
    Embedded(Cursor<&'static [u8]>),
    Filesystem(std::io::BufReader<std::fs::File>),
}

/// Open the audio input for a source. Fails cleanly (string error, no panic)
/// on missing/unreadable custom paths.
fn open_media(source: &AthanSource) -> Result<MediaSource, String> {
    match source {
        AthanSource::Builtin => Ok(MediaSource::Embedded(Cursor::new(ATHAN_MP3))),
        AthanSource::File(path) => {
            // File::open succeeds on directories on Linux; reject them here
            // so the error names the problem instead of a decoder failure.
            if path.is_dir() {
                return Err(format!("Custom athan {} is a directory", path.display()));
            }
            std::fs::File::open(path)
                .map(|f| MediaSource::Filesystem(std::io::BufReader::new(f)))
                .map_err(|e| {
                    format!("Could not open custom athan {}: {e}", path.display())
                })
        }
    }
}

fn play_once(mixer: &Mixer, source: &AthanSource, volume: f32) -> Result<Player, String> {
    let player = Player::connect_new(mixer);
    match open_media(source)? {
        MediaSource::Embedded(reader) => {
            let decoded = Decoder::new(reader)
                .map_err(|e| format!("Could not decode embedded athan.mp3: {e}"))?;
            player.append(decoded);
        }
        MediaSource::Filesystem(reader) => {
            let decoded = Decoder::new(reader)
                .map_err(|e| format!("Could not decode custom athan: {e}"))?;
            player.append(decoded);
        }
    }
    player.set_volume(volume);
    Ok(player)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_without_playback_is_a_noop() {
        stop_athan();
        assert!(!is_playing());
    }

    #[test]
    fn volume_percent_maps_and_clamps() {
        assert_eq!(effective_volume(None), 1.0);
        assert_eq!(effective_volume(Some(0)), 0.0);
        assert!((effective_volume(Some(50)) - 0.5).abs() < 1e-6);
        assert_eq!(effective_volume(Some(100)), 1.0);
        // Out-of-range values clamp instead of amplifying past full volume.
        assert_eq!(effective_volume(Some(255)), 1.0);
    }

    #[test]
    fn missing_or_directory_custom_paths_fail_cleanly() {
        // Both must produce an error (never a panic) before any playback
        // is attempted — a hostile config can name any path.
        let missing = AthanSource::File(PathBuf::from("/nonexistent/mawaqit/athan.mp3"));
        let dir = AthanSource::File(std::env::temp_dir());
        for source in [&missing, &dir] {
            assert!(open_media(source).is_err(), "{source:?} must fail to open");
        }
        // The embedded source always opens, wherever the app runs from.
        assert!(open_media(&AthanSource::Builtin).is_ok());
    }

    #[test]
    fn junk_file_with_audio_extension_fails_to_decode_not_panic() {
        let path = std::env::temp_dir().join(format!(
            "mawaqit-junk-athan-{}.mp3",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::write(&path, [0xFFu8, 0x00, b'M', b'Z', 0x01, 0x02]).unwrap();
        let decoded =
            open_media(&AthanSource::File(path.clone())).and_then(|media| match media {
                MediaSource::Filesystem(reader) => {
                    Decoder::new(reader).map(|_| ()).map_err(|e| e.to_string())
                }
                MediaSource::Embedded(_) => Err("unexpected embedded".into()),
            });
        let _ = std::fs::remove_file(&path);
        assert!(decoded.is_err(), "junk bytes must fail to decode, never panic");
    }
}
