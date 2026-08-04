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
use tokio::sync::mpsc::{error::TrySendError, Sender};
use uuid::Uuid;

/// Responses that may queue for one connection before it is considered stuck.
///
/// This lane used to be unbounded, which meant a client that joined and then
/// stopped reading its socket accumulated every broadcast on the server's heap
/// with nothing to stop it — the cheapest possible memory exhaustion against a
/// community server. Bounded, a stuck client is disconnected instead: it has
/// already missed messages, and `FetchHistory` is how it catches up on
/// reconnect.
pub const BROADCAST_QUEUE_DEPTH: usize = 256;

struct Conn {
    member: Uuid,
    tx: Sender<PartyResponse>,
}

/// Registry of connected clients' outbound senders, keyed by connection id and
/// tagged with the joined member id (a member may have more than one connection).
#[derive(Default)]
pub struct Hub {
    conns: Mutex<HashMap<Uuid, Conn>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a connection's outbound sender for a joined `member`. Called once
    /// the connection has joined so unauthenticated peers receive nothing.
    pub fn register(&self, conn_id: Uuid, member: Uuid, tx: Sender<PartyResponse>) {
        self.conns
            .lock()
            .unwrap()
            .insert(conn_id, Conn { member, tx });
    }

    /// Remove a connection (on disconnect).
    pub fn unregister(&self, conn_id: Uuid) {
        self.conns.lock().unwrap().remove(&conn_id);
    }

    /// Push `resp` to every connection except `except` (the originator). Dead
    /// senders (closed receivers) are ignored; they are cleaned up on unregister.
    /// A connection whose queue is full has stopped draining its socket and is
    /// dropped from the hub — see [`BROADCAST_QUEUE_DEPTH`].
    pub fn broadcast_except(&self, except: Uuid, resp: PartyResponse) {
        let mut conns = self.conns.lock().unwrap();
        let mut stuck: Vec<Uuid> = Vec::new();
        for (id, conn) in conns.iter() {
            if *id != except {
                Self::push(*id, conn, resp.clone(), &mut stuck);
            }
        }
        for id in stuck {
            conns.remove(&id);
        }
    }

    /// Push `resp` to every connection belonging to `member` (DM delivery).
    pub fn send_to_member(&self, member: Uuid, resp: PartyResponse) {
        let mut conns = self.conns.lock().unwrap();
        let mut stuck: Vec<Uuid> = Vec::new();
        for (id, conn) in conns.iter() {
            if conn.member == member {
                Self::push(*id, conn, resp.clone(), &mut stuck);
            }
        }
        for id in stuck {
            conns.remove(&id);
        }
    }

    /// Non-blocking send. The critical section holds a `std::sync::Mutex` and
    /// must never await, so a full queue is treated as "this client is not
    /// reading" rather than something to wait on.
    fn push(id: Uuid, conn: &Conn, resp: PartyResponse, stuck: &mut Vec<Uuid>) {
        match conn.tx.try_send(resp) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::warn!(conn = %id, "dropping a connection that stopped reading its socket");
                stuck.push(id);
            }
            // The receiver is gone; `unregister` will clean it up.
            Err(TrySendError::Closed(_)) => {}
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
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (member_a, member_b) = (Uuid::new_v4(), Uuid::new_v4());
        let (a_tx, mut a_rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        let (b_tx, mut b_rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        hub.register(a, member_a, a_tx);
        hub.register(b, member_b, b_tx);
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

    #[test]
    fn send_to_member_targets_only_that_member() {
        let hub = Hub::new();
        let member_a = Uuid::new_v4();
        let member_b = Uuid::new_v4();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        let (b_tx, mut b_rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        hub.register(Uuid::new_v4(), member_a, a_tx);
        hub.register(Uuid::new_v4(), member_b, b_tx);

        hub.send_to_member(member_b, PartyResponse::Error("dm".to_string()));
        assert!(a_rx.try_recv().is_err(), "member A must not receive B's DM");
        assert!(matches!(
            b_rx.try_recv(),
            Ok(PartyResponse::Error(ref m)) if m == "dm"
        ));
    }

    /// A joined client that stops reading its socket must not be able to grow
    /// the server's heap without bound. Once its queue fills it is dropped.
    #[test]
    fn a_client_that_stops_reading_is_dropped_rather_than_buffered_forever() {
        let hub = Hub::new();
        let member = Uuid::new_v4();
        let conn_id = Uuid::new_v4();
        // Receiver is held but never drained.
        let (tx, _rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        hub.register(conn_id, member, tx);

        for _ in 0..BROADCAST_QUEUE_DEPTH {
            hub.broadcast_except(Uuid::new_v4(), PartyResponse::Error("spam".to_string()));
        }
        assert_eq!(hub.len(), 1, "still registered while the queue has room");

        // The next push finds the queue full and evicts the connection.
        hub.broadcast_except(Uuid::new_v4(), PartyResponse::Error("spam".to_string()));
        assert!(
            hub.is_empty(),
            "a connection that never drains must be dropped, not buffered"
        );
    }
}
