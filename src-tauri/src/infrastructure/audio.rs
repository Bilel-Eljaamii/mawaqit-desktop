use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

/// Embedded so the adhan plays no matter where the app is launched from.
const ATHAN_MP3: &[u8] = include_bytes!("../../assets/athan.mp3");

/// How often the audio thread notices that the athan finished on its own.
const PLAYBACK_POLL: Duration = Duration::from_millis(250);

enum Command {
    Play,
    Stop,
}

static COMMANDS: OnceLock<Sender<Command>> = OnceLock::new();
static PLAYING: AtomicBool = AtomicBool::new(false);

/// Start playing the athan (non-blocking). Returns false when the audio
/// thread is gone. A playback already running is replaced. The playing flag
/// is set here so [`is_playing`] is true as soon as this returns; the audio
/// thread clears it if the device is unavailable or playback ends/stops.
pub fn play_athan() -> bool {
    let sender = COMMANDS.get_or_init(spawn_audio_thread);
    PLAYING.store(true, Ordering::SeqCst);
    if sender.send(Command::Play).is_err() {
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

/// Dedicated thread owning the audio device: rodio's `Sink` is neither Send
/// nor Clone, so playback state lives here and control goes over a channel.
fn spawn_audio_thread() -> Sender<Command> {
    let (tx, rx) = mpsc::channel::<Command>();
    thread::spawn(move || {
        let Ok((_stream, handle)) = OutputStream::try_default() else {
            eprintln!("No audio output device available for athan playback");
            PLAYING.store(false, Ordering::SeqCst);
            return;
        };

        let mut current: Option<Sink> = None;
        loop {
            match rx.recv_timeout(PLAYBACK_POLL) {
                Ok(Command::Play) => {
                    if let Some(sink) = &current {
                        sink.stop();
                    }
                    match play_once(&handle) {
                        Ok(sink) => current = Some(sink),
                        Err(e) => {
                            eprintln!("{e}");
                            PLAYING.store(false, Ordering::SeqCst);
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

fn play_once(handle: &OutputStreamHandle) -> Result<Sink, String> {
    let sink =
        Sink::try_new(handle).map_err(|e| format!("Could not open audio sink: {e}"))?;
    let source = Decoder::new(Cursor::new(ATHAN_MP3))
        .map_err(|e| format!("Could not decode embedded athan.mp3: {e}"))?;
    sink.append(source);
    Ok(sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_without_playback_is_a_noop() {
        stop_athan();
        assert!(!is_playing());
    }
}
