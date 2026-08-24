//! In-process PCM broadcast bus (spec §3).
//!
//! Captured audio is published here and read directly by the transcription module —
//! it NEVER crosses into the webview. Routing PCM through IPC would cost a base64
//! round trip per frame for no benefit; the whole point of the bus is to keep the
//! hot audio path inside Rust.
//!
//! Backpressure: if the transcription consumer stalls, we must not grow memory
//! unbounded. A tokio broadcast channel drops the OLDEST frames and reports the count
//! to the lagging receiver — exactly the right policy for live audio (a stale frame is
//! worthless), and we surface that count so the UI can show "N frames dropped".

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// A frame of 24 kHz mono PCM. `Arc` so a frame is broadcast to N consumers without
/// copying the samples.
pub type PcmFrame = Arc<Vec<f32>>;

#[derive(Clone)]
pub struct AudioBus {
    tx: broadcast::Sender<PcmFrame>,
    dropped: Arc<AtomicU64>,
    published: Arc<AtomicU64>,
}

impl AudioBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self {
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
            published: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Publish a frame. Returns the number of live receivers (0 is fine — capture may
    /// run before transcription subscribes).
    pub fn publish(&self, frame: PcmFrame) -> usize {
        self.published.fetch_add(1, Ordering::Relaxed);
        self.tx.send(frame).unwrap_or(0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PcmFrame> {
        self.tx.subscribe()
    }

    /// Record that `n` frames were dropped for a lagging consumer.
    pub fn note_dropped(&self, n: u64) {
        self.dropped.fetch_add(n, Ordering::Relaxed);
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    /// Receive the next frame, counting any frames the broadcast channel dropped
    /// because this consumer fell behind. Returns None when the sender is gone.
    pub async fn recv_counting(&self, rx: &mut broadcast::Receiver<PcmFrame>) -> Option<PcmFrame> {
        loop {
            match rx.recv().await {
                Ok(frame) => return Some(frame),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // The oldest `n` frames were overwritten before we read them.
                    self.note_dropped(n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn counts_dropped_frames_when_consumer_lags() {
        // Publish more than the channel can hold without receiving; the overflow is
        // dropped and counted. The exact split depends on tokio's ring sizing, so we
        // assert the invariant: every published frame was either received or dropped.
        let bus = AudioBus::new(2);
        let mut rx = bus.subscribe();
        for i in 0..5 {
            bus.publish(Arc::new(vec![i as f32]));
        }
        let mut received = 0u64;
        loop {
            match rx.try_recv() {
                Ok(_) => received += 1,
                Err(broadcast::error::TryRecvError::Lagged(n)) => bus.note_dropped(n),
                Err(_) => break, // empty / closed
            }
        }
        assert!(bus.dropped() >= 1, "some frames must have been dropped");
        assert_eq!(bus.dropped() + received, 5, "received + dropped == published");
        assert_eq!(bus.published(), 5);
    }

    #[tokio::test]
    async fn publish_without_subscribers_is_ok() {
        let bus = AudioBus::new(4);
        assert_eq!(bus.publish(Arc::new(vec![0.0])), 0);
    }

    #[tokio::test]
    async fn delivers_frames_in_order_when_keeping_up() {
        let bus = AudioBus::new(8);
        let mut rx = bus.subscribe();
        bus.publish(Arc::new(vec![1.0]));
        bus.publish(Arc::new(vec![2.0]));
        assert_eq!(bus.recv_counting(&mut rx).await.unwrap()[0], 1.0);
        assert_eq!(bus.recv_counting(&mut rx).await.unwrap()[0], 2.0);
        assert_eq!(bus.dropped(), 0);
    }
}
