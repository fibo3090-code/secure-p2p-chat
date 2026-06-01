//! Connection hub for broadcast fan-out (Phase 1, slice 3).
//!
//! Each live connection registers an outbound channel keyed by a unique connection
//! id. When a member posts a message, the runtime pushes the resulting
//! [`PartyResponse::Message`] to every *other* registered connection, which then
//! writes it down its own tunnel. Uses a `std::sync::Mutex` because the critical
//! section never awaits.

use std::collections::HashMap;
use std::sync::Mutex;

use messenger_core::party::PartyResponse;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// Registry of connected clients' outbound senders.
#[derive(Default)]
pub struct Hub {
    conns: Mutex<HashMap<Uuid, UnboundedSender<PartyResponse>>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a connection's outbound sender. Called once the connection has a
    /// joined member so unauthenticated peers do not receive broadcasts.
    pub fn register(&self, conn_id: Uuid, tx: UnboundedSender<PartyResponse>) {
        self.conns.lock().unwrap().insert(conn_id, tx);
    }

    /// Remove a connection (on disconnect).
    pub fn unregister(&self, conn_id: Uuid) {
        self.conns.lock().unwrap().remove(&conn_id);
    }

    /// Push `resp` to every connection except `except` (the originator). Dead
    /// senders (closed receivers) are ignored; they are cleaned up on unregister.
    pub fn broadcast_except(&self, except: Uuid, resp: PartyResponse) {
        let conns = self.conns.lock().unwrap();
        for (id, tx) in conns.iter() {
            if *id != except {
                let _ = tx.send(resp.clone());
            }
        }
    }

    /// Number of registered connections (test/diagnostic helper).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.conns.lock().unwrap().len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_reaches_others_but_not_the_originator() {
        let hub = Hub::new();
        assert!(hub.is_empty());
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        hub.register(a, a_tx);
        hub.register(b, b_tx);
        assert_eq!(hub.len(), 2);

        hub.broadcast_except(a, PartyResponse::Error("ping".to_string()));
        // a (originator) gets nothing; b receives it.
        assert!(a_rx.try_recv().is_err());
        assert!(matches!(
            b_rx.try_recv(),
            Ok(PartyResponse::Error(ref m)) if m == "ping"
        ));

        hub.unregister(b);
        assert_eq!(hub.len(), 1);
    }
}
