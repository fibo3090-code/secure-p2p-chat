//! Connection hub for broadcast fan-out: each joined connection registers its
//! outbound sender under a connection id, and the runtime pushes responses to
//! the others through it. A `std::sync::Mutex`: the critical section never awaits.

use std::collections::HashMap;
use std::sync::Mutex;

use messenger_core::party::PartyResponse;
use tokio::sync::mpsc::{error::TrySendError, Sender};
use uuid::Uuid;

/// Responses that may queue for one connection before it is considered stuck.
///
/// Bounded, so a client that stopped reading cannot grow the server's heap: it
/// is evicted and catches up via `FetchHistory` on reconnect. The hub holds the
/// connection's *only* sender, so eviction also ends the connection task (see
/// `connection::serve_connection_with`).
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

    /// Push `resp` to every connection except `except` (the originator),
    /// evicting any whose queue is full (see [`BROADCAST_QUEUE_DEPTH`]).
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

    /// Push `resp` to every connection belonging to `member` *except* `except`
    /// (DM delivery; [`Uuid::nil`] reaches all of them). Excluding the
    /// originating connection reaches the sender's other devices without
    /// duplicating the row it already appended.
    pub fn send_to_member_except(&self, member: Uuid, except: Uuid, resp: PartyResponse) {
        let mut conns = self.conns.lock().unwrap();
        let mut stuck: Vec<Uuid> = Vec::new();
        for (id, conn) in conns.iter() {
            if conn.member == member && *id != except {
                Self::push(*id, conn, resp.clone(), &mut stuck);
            }
        }
        for id in stuck {
            conns.remove(&id);
        }
    }

    /// Push `resp` to every registered connection, including the originator
    /// (directory refreshes).
    pub fn broadcast_all(&self, resp: PartyResponse) {
        self.broadcast_except(Uuid::nil(), resp);
    }

    /// Whether `member` still has at least one registered connection (presence
    /// is per member, connections per device).
    pub fn member_is_connected(&self, member: Uuid) -> bool {
        self.conns
            .lock()
            .unwrap()
            .values()
            .any(|c| c.member == member)
    }

    /// Non-blocking send: the critical section must never await, so a full
    /// queue means "this client is not reading".
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

        hub.send_to_member_except(
            member_b,
            Uuid::nil(),
            PartyResponse::Error("dm".to_string()),
        );
        assert!(a_rx.try_recv().is_err(), "member A must not receive B's DM");
        assert!(matches!(
            b_rx.try_recv(),
            Ok(PartyResponse::Error(ref m)) if m == "dm"
        ));
    }

    /// A DM has to reach the sender's *other* devices without being duplicated
    /// on the one that sent it (which already showed it optimistically).
    #[test]
    fn send_to_member_except_reaches_other_devices_but_not_the_sending_one() {
        let hub = Hub::new();
        let member = Uuid::new_v4();
        let (phone, laptop) = (Uuid::new_v4(), Uuid::new_v4());
        let (phone_tx, mut phone_rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        let (laptop_tx, mut laptop_rx) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        hub.register(phone, member, phone_tx);
        hub.register(laptop, member, laptop_tx);

        // The phone sent the DM, so only the laptop should be told about it.
        hub.send_to_member_except(member, phone, PartyResponse::Error("dm".to_string()));
        assert!(
            phone_rx.try_recv().is_err(),
            "the sending device already has this message"
        );
        assert!(matches!(
            laptop_rx.try_recv(),
            Ok(PartyResponse::Error(ref m)) if m == "dm"
        ));
    }

    /// Presence is per-member, connections are per-device: a member with two
    /// clients open is still connected after one of them closes.
    #[test]
    fn a_member_stays_connected_while_any_of_their_devices_remains() {
        let hub = Hub::new();
        let member = Uuid::new_v4();
        let (phone, laptop) = (Uuid::new_v4(), Uuid::new_v4());
        let (phone_tx, _p) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        let (laptop_tx, _l) = tokio::sync::mpsc::channel(BROADCAST_QUEUE_DEPTH);
        hub.register(phone, member, phone_tx);
        hub.register(laptop, member, laptop_tx);

        hub.unregister(phone);
        assert!(
            hub.member_is_connected(member),
            "the laptop is still connected, so the member is not offline"
        );
        hub.unregister(laptop);
        assert!(!hub.member_is_connected(member));
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
