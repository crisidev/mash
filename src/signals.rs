use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub(crate) enum SignalEvent {
    Winch,
    Int,
    Tstp,
}

pub(crate) async fn signal_listener(tx: mpsc::Sender<SignalEvent>) {
    let mut sigwinch = signal(SignalKind::window_change()).expect("Failed to register SIGWINCH");
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT");

    // SIGTSTP needs special handling - we register for it
    let mut sigtstp =
        signal(SignalKind::from_raw(nix::sys::signal::Signal::SIGTSTP as i32)).expect("Failed to register SIGTSTP");

    loop {
        tokio::select! {
            _ = sigwinch.recv() => {
                let _ = tx.send(SignalEvent::Winch).await;
            }
            _ = sigint.recv() => {
                let _ = tx.send(SignalEvent::Int).await;
            }
            _ = sigtstp.recv() => {
                let _ = tx.send(SignalEvent::Tstp).await;
            }
        }
    }
}
