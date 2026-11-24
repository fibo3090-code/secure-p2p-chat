//! Party request dispatcher: the pure, network-free protocol layer.
//!
//! Given a connection's session state and a [`PartyRequest`], it applies the
//! request to the shared [`PartyState`] and produces the [`PartyResponse`]s to
//! send back and fan out, enforcing "join before acting". The runtime
//! (`connection.rs`) owns transport, fan-out and deferred I/O.

use messenger_core::party::{
    ChannelKind, Envelope, PartyRequest, PartyResponse, UploadTarget, MAX_INLINE_FILE_BYTES,
    PARTY_CHUNK_BYTES, UNKNOWN_FILE_ERROR,
};
use uuid::Uuid;

use crate::state::{BlobReadPlan, PartyState, StagedUpload};

/// The outcome of handling one request: `replies` go back to the requester,
/// `broadcast` to every *other* connection, `directed` to one member's
/// connections (via the hub).
#[derive(Debug, Default)]
pub struct Dispatch {
    pub replies: Vec<PartyResponse>,
    pub broadcast: Vec<PartyResponse>,
    pub directed: Vec<(Uuid, PartyResponse)>,
    /// Work the runtime must do **after** releasing the state lock (a blob read
    /// or write, up to `MAX_PARTY_FILE_BYTES`), which produces further replies
    /// of its own. Deciding *whether* it may happen stays here.
    pub deferred: Option<Deferred>,
}

/// I/O the runtime performs once the state lock is released.
#[derive(Debug)]
pub enum Deferred {
    /// Read an authorised blob and answer with its bytes.
    ///
    /// `offset` distinguishes the two download shapes: `None` is `DownloadFile`
    /// (answer `FileData`), `Some` is `DownloadChunk` (answer `FileChunk`).
    Download {
        hash: String,
        offset: Option<u64>,
        plan: BlobReadPlan,
    },
    /// Write a staged upload's bytes, then commit it under the lock again.
    Upload(Box<StagedUpload>),
}

impl Dispatch {
    /// A single reply to the requester, and nothing else.
    pub fn reply(resp: PartyResponse) -> Self {
        Self {
            replies: vec![resp],
            ..Self::default()
        }
    }

    /// A dispatch whose whole effect happens after the lock is released.
    fn defer(work: Deferred) -> Self {
        Self {
            deferred: Some(work),
            ..Self::default()
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self::reply(PartyResponse::Error(msg.into()))
    }

    /// `Ok` for a successful administrative action, `Error` for a refused one.
    fn outcome(result: Result<String, String>) -> Self {
        match result {
            Ok(msg) => Self::reply(PartyResponse::Ok(msg)),
            Err(e) => Self::error(e),
        }
    }

    /// A send the client is already showing was refused. `ActionFailed`, not
    /// `Error`, so the client can take that optimistic row back off the screen;
    /// `thread` is its channel or DM thread (`nil` when unknown).
    pub fn action_failed(thread: Uuid, reason: String) -> Self {
        Self::reply(PartyResponse::ActionFailed {
            channel: thread,
            reason,
        })
    }
}

/// Turn the result of upload phase 3 into the replies and fan-out it produces.
pub fn upload_committed(
    state: &PartyState,
    member: Uuid,
    result: Result<(Envelope, UploadTarget), String>,
) -> Dispatch {
    match result {
        // A file fans out exactly like a text post.
        Ok((env, target)) => posted(state, member, target, env),
        Err(reason) => Dispatch::action_failed(Uuid::nil(), reason),
    }
}

/// Turn a performed download into the reply it answers.
pub fn download_reply(
    hash: String,
    offset: Option<u64>,
    total: u64,
    bytes: Option<Vec<u8>>,
) -> Dispatch {
    match (bytes, offset) {
        (Some(data), None) => Dispatch::reply(PartyResponse::FileData { hash, data }),
        (Some(data), Some(offset)) => Dispatch::reply(PartyResponse::FileChunk {
            hash,
            offset,
            total,
            data,
        }),
        // Gone since access was granted: answer as for an unknown hash.
        (None, _) => unknown_file(),
    }
}

/// The answer to a download of a file that does not exist *or* that the member
/// may not see — deliberately the same, so the endpoint never confirms that a
/// file it will not hand over exists.
fn unknown_file() -> Dispatch {
    Dispatch::error(UNKNOWN_FILE_ERROR)
}

/// How many rejected `Join` attempts a single connection may make before it is
/// refused outright, so one handshake cannot buy unlimited password guesses
/// (reconnecting is then bounded by the accept loop's per-IP limiter).
pub const MAX_JOIN_ATTEMPTS: u32 = 3;

/// Per-connection session state. A connection must `Join` before any other
/// request is honoured; once joined it carries the member's id. `peer_fingerprint`
/// is the identity verified during the v3 handshake, bound to the member on join.
#[derive(Debug, Default)]
pub struct ConnState {
    member: Option<Uuid>,
    peer_fingerprint: Option<String>,
    /// Rejected join attempts on this connection (see [`MAX_JOIN_ATTEMPTS`]).
    join_failures: u32,
    /// Uploads (and staged reservations) this connection has in flight, so a
    /// disconnect can release them.
    uploads: Vec<Uuid>,
}

impl ConnState {
    /// Create connection state carrying the handshake-verified peer fingerprint.
    pub fn with_fingerprint(fingerprint: String) -> Self {
        Self {
            peer_fingerprint: Some(fingerprint),
            ..Self::default()
        }
    }

    /// Uploads and staged reservations this connection has in flight, for
    /// cleanup on disconnect. Both are keyed by the same id.
    pub fn open_uploads(&self) -> &[Uuid] {
        &self.uploads
    }

    /// Forget one in-flight id, once it has been committed or discarded.
    pub fn finish_upload(&mut self, upload: Uuid) {
        self.uploads.retain(|u| *u != upload);
    }

    /// Record an inline upload's minted id: it holds a reservation that a
    /// disconnect must release.
    pub fn track_upload(&mut self, upload: Uuid) {
        self.uploads.push(upload);
    }

    /// The joined member's id, if this connection has completed `Join`.
    pub fn member(&self) -> Option<Uuid> {
        self.member
    }
}

/// Ack a stored post and deliver it to everyone else who should see it — the
/// channel's readers, or a DM's two ends.
fn posted(state: &PartyState, member: Uuid, target: UploadTarget, env: Envelope) -> Dispatch {
    match target {
        UploadTarget::Channel(_) => channel_fanout(state, env),
        UploadTarget::Dm(to) => Dispatch {
            replies: vec![PartyResponse::MessagePosted {
                channel: env.channel,
                seq: env.seq,
            }],
            directed: dm_delivery(member, to, env),
            ..Dispatch::default()
        },
    }
}

/// Where a stored DM envelope has to be delivered: the recipient, and the
/// sender's *other* connections (the runtime skips the originating one, which
/// appended it optimistically). A self-DM is delivered once.
fn dm_delivery(from: Uuid, to: Uuid, env: Envelope) -> Vec<(Uuid, PartyResponse)> {
    if from == to {
        return vec![(to, PartyResponse::Message(env))];
    }
    vec![
        (to, PartyResponse::Message(env.clone())),
        (from, PartyResponse::Message(env)),
    ]
}

/// How a stored channel message reaches everyone else: broadcast when every
/// member may read the channel, otherwise directed to exactly its readers (a
/// broadcast would leak a private channel's messages).
fn channel_fanout(state: &PartyState, env: Envelope) -> Dispatch {
    let ack = PartyResponse::MessagePosted {
        channel: env.channel,
        seq: env.seq,
    };
    if state.channel_is_open_to_all(env.channel) {
        return Dispatch {
            replies: vec![ack],
            broadcast: vec![PartyResponse::Message(env)],
            ..Dispatch::default()
        };
    }
    let readers = state.members_who_can_read(env.channel);
    Dispatch {
        replies: vec![ack],
        directed: readers
            .into_iter()
            .map(|m| (m, PartyResponse::Message(env.clone())))
            .collect(),
        ..Dispatch::default()
    }
}

/// The thread a failed upload belongs to, so `ActionFailed` can be correlated
/// with the message the client is already showing.
fn upload_thread(member: Uuid, target: UploadTarget) -> Uuid {
    match target {
        UploadTarget::Channel(id) => id,
        UploadTarget::Dm(to) => messenger_core::party::dm_thread_id(member, to),
    }
}

/// Reply with the requester's own filtered channel list and nudge everyone
/// else to re-fetch theirs: `channels_for` filters private channels per member,
/// so one member's view must never be broadcast.
fn channel_list_refresh(state: &PartyState, member: Uuid) -> Dispatch {
    Dispatch {
        replies: vec![PartyResponse::Channels(state.channels_for(member))],
        broadcast: vec![PartyResponse::DirectoryChanged],
        ..Dispatch::default()
    }
}

/// A channel-directory change that also carries a confirmation message.
fn channel_change(state: &PartyState, member: Uuid, result: Result<String, String>) -> Dispatch {
    match result {
        Ok(msg) => {
            let mut out = channel_list_refresh(state, member);
            out.replies.push(PartyResponse::Ok(msg));
            out
        }
        Err(e) => Dispatch::error(e),
    }
}

/// Phase 1 of an inline upload (`PostFile` / `SendFileDm`): check it and take
/// the bytes, deferring the write. The minted id is tracked on the connection
/// because its storage reservation must be released on disconnect.
fn begin_inline_upload(
    state: &mut PartyState,
    conn: &mut ConnState,
    member: Uuid,
    target: UploadTarget,
    name: String,
    mime: String,
    data: Vec<u8>,
) -> Dispatch {
    match state.begin_inline_upload(member, target, name, mime, data) {
        Ok(staged) => {
            conn.track_upload(staged.id());
            Dispatch::defer(Deferred::Upload(Box::new(staged)))
        }
        Err(reason) => Dispatch::action_failed(upload_thread(member, target), reason),
    }
}

/// Authorise a download (whole file when `offset` is `None`, one chunk
/// otherwise); the read itself happens after the lock is released.
fn plan_download(state: &PartyState, member: Uuid, hash: String, offset: Option<u64>) -> Dispatch {
    match state.plan_blob_read(member, &hash, offset) {
        // A whole-file read is one frame; past the inline limit it would read
        // up to 100 MiB into memory and then fail the frame limit, taking the
        // connection with it (SRV-14). Decided after the access check, so the
        // refusal says nothing about a file this member cannot see.
        Some(plan) if offset.is_none() && plan.total() > MAX_INLINE_FILE_BYTES as u64 => {
            Dispatch::reply(PartyResponse::Error(
                "this file is too large to download in one piece; download it in chunks"
                    .to_string(),
            ))
        }
        Some(plan) => Dispatch::defer(Deferred::Download { hash, offset, plan }),
        None => unknown_file(),
    }
}

/// Apply `req` to `state` for the connection `conn`, returning the responses to
/// send back to that client. Newly posted messages are stored in `state`; the
/// runtime is responsible for broadcasting them to other connections.
///
/// A connection must `Join` before anything else is honoured, and may join
/// only once.
pub fn handle_request(state: &mut PartyState, conn: &mut ConnState, req: PartyRequest) -> Dispatch {
    match (conn.member, req) {
        (None, PartyRequest::Join { username, password }) => {
            join(state, conn, &username, password.as_deref())
        }
        (None, _) => Dispatch::error("join required"),
        (Some(member), req) => handle_member_request(state, conn, member, req),
    }
}

fn join(
    state: &mut PartyState,
    conn: &mut ConnState,
    username: &str,
    password: Option<&str>,
) -> Dispatch {
    // A few rejected attempts per connection, then it is spent.
    if conn.join_failures >= MAX_JOIN_ATTEMPTS {
        return Dispatch::reply(PartyResponse::JoinRejected {
            reason: "too many failed attempts on this connection — reconnect to try again"
                .to_string(),
        });
    }
    match state.join(username, password, conn.peer_fingerprint.clone()) {
        Ok(id) => {
            conn.member = Some(id);
            Dispatch {
                replies: vec![PartyResponse::Joined {
                    member_id: id,
                    server_name: state.name().to_string(),
                    tier: state.tier(),
                }],
                // Everyone else's directory just went stale.
                broadcast: vec![PartyResponse::Members(state.members())],
                ..Dispatch::default()
            }
        }
        Err(e) => {
            conn.join_failures = conn.join_failures.saturating_add(1);
            Dispatch::reply(PartyResponse::JoinRejected {
                reason: e.reason().to_string(),
            })
        }
    }
}

/// The text bytes a request writes, if it is a write at all. Every state
/// change counts, governance included (each costs an audit row); reads and the
/// chunks of an already admitted upload are free.
fn write_cost(req: &PartyRequest) -> Option<usize> {
    match req {
        PartyRequest::PostMessage { text, .. } | PartyRequest::SendDm { text, .. } => {
            Some(text.len())
        }
        PartyRequest::PostFile { .. }
        | PartyRequest::SendFileDm { .. }
        | PartyRequest::StartUpload { .. }
        | PartyRequest::ShareFile { .. }
        | PartyRequest::CreateChannel { .. }
        | PartyRequest::CreateChannelOfKind { .. }
        | PartyRequest::DeleteChannel { .. }
        | PartyRequest::SetChannelAccess { .. }
        | PartyRequest::SetRole { .. }
        | PartyRequest::DeleteFile { .. }
        | PartyRequest::SetFilePermissions { .. } => Some(0),
        _ => None,
    }
}

/// The answer to a write refused by the rate limit: `ActionFailed` for a send
/// the client is showing (so the row is retracted and its FIFO stays in step),
/// `Error` otherwise.
fn rate_limited(member: Uuid, req: &PartyRequest, reason: String) -> Dispatch {
    match req {
        PartyRequest::PostMessage { channel, .. } | PartyRequest::PostFile { channel, .. } => {
            Dispatch::action_failed(*channel, reason)
        }
        PartyRequest::SendDm { to, .. } | PartyRequest::SendFileDm { to, .. } => {
            Dispatch::action_failed(upload_thread(member, UploadTarget::Dm(*to)), reason)
        }
        PartyRequest::StartUpload { target, .. } | PartyRequest::ShareFile { target, .. } => {
            Dispatch::action_failed(upload_thread(member, *target), reason)
        }
        _ => Dispatch::error(reason),
    }
}

/// Every request a joined member may make.
fn handle_member_request(
    state: &mut PartyState,
    conn: &mut ConnState,
    member: Uuid,
    req: PartyRequest,
) -> Dispatch {
    if let Some(bytes) = write_cost(&req) {
        if let Err(reason) = state.check_write_rate(member, bytes) {
            return rate_limited(member, &req, reason);
        }
    }
    match req {
        PartyRequest::Join { .. } => Dispatch::error("already joined on this connection"),
        PartyRequest::ListMembers => Dispatch::reply(PartyResponse::Members(state.members())),
        // Filtered per member.
        PartyRequest::ListChannels => {
            Dispatch::reply(PartyResponse::Channels(state.channels_for(member)))
        }
        PartyRequest::PostMessage { channel, text } => {
            match state.post_message(member, channel, text) {
                // Ack the poster; deliver to everyone else who may read the
                // channel (see `channel_fanout`).
                Ok(env) => channel_fanout(state, env),
                Err(e) => Dispatch::action_failed(channel, e),
            }
        }
        PartyRequest::FetchHistory { channel, since_seq } => Dispatch::reply(
            PartyResponse::History(state.history_since(member, channel, since_seq)),
        ),
        PartyRequest::SendDm { to, text } => match state.post_dm(member, to, text) {
            // Ack the sender (who appends locally); deliver to the recipient and
            // to the sender's *other* devices.
            Ok(env) => posted(state, member, UploadTarget::Dm(to), env),
            Err(e) => Dispatch::action_failed(upload_thread(member, UploadTarget::Dm(to)), e),
        },
        PartyRequest::FetchDmHistory { with, since_seq } => {
            let thread = messenger_core::party::dm_thread_id(member, with);
            Dispatch::reply(PartyResponse::History(state.dm_history(thread, since_seq)))
        }
        PartyRequest::PostFile {
            channel,
            name,
            mime,
            data,
        } => begin_inline_upload(
            state,
            conn,
            member,
            UploadTarget::Channel(channel),
            name,
            mime,
            data,
        ),
        PartyRequest::SendFileDm {
            to,
            name,
            mime,
            data,
        } => begin_inline_upload(state, conn, member, UploadTarget::Dm(to), name, mime, data),
        PartyRequest::DownloadFile { hash } => plan_download(state, member, hash, None),
        PartyRequest::DownloadChunk { hash, offset } => {
            plan_download(state, member, hash, Some(offset))
        }

        // --- Governance and file management ---------------------------------
        // Channel changes answer with the requester's own filtered list (see
        // `channel_list_refresh`).
        PartyRequest::CreateChannel { name } => {
            create_channel(state, member, &name, ChannelKind::Public, vec![])
        }
        PartyRequest::CreateChannelOfKind {
            name,
            kind,
            members,
        } => create_channel(state, member, &name, kind, members),
        PartyRequest::DeleteChannel { channel } => {
            let result = state.delete_channel(member, channel);
            channel_change(state, member, result)
        }
        PartyRequest::SetChannelAccess {
            channel,
            kind,
            members,
        } => {
            let result = state.set_channel_access(member, channel, kind, members);
            channel_change(state, member, result)
        }
        PartyRequest::SetRole {
            member: target,
            role,
        } => match state.set_role(member, target, role) {
            Ok(msg) => Dispatch {
                // A role change can change what the member may *see* too, so
                // refresh their channel list as well.
                replies: vec![
                    PartyResponse::Members(state.members()),
                    PartyResponse::Channels(state.channels_for(member)),
                    PartyResponse::Ok(msg),
                ],
                broadcast: vec![
                    PartyResponse::Members(state.members()),
                    PartyResponse::DirectoryChanged,
                ],
                ..Dispatch::default()
            },
            Err(e) => Dispatch::error(e),
        },
        PartyRequest::ListFiles => Dispatch::reply(PartyResponse::Files(state.files_for(member))),
        PartyRequest::ListFilesPage { offset } => {
            let (files, next) = state.files_page(member, offset as usize);
            Dispatch::reply(PartyResponse::FilesPage {
                files,
                offset,
                next: next.and_then(|n| u32::try_from(n).ok()),
            })
        }
        PartyRequest::DeleteFile { hash, channel } => {
            Dispatch::outcome(state.delete_file(member, &hash, channel))
        }
        PartyRequest::SetFilePermissions {
            hash,
            location,
            member: target,
            perms,
        } => Dispatch::outcome(state.set_file_permissions(member, &hash, location, target, perms)),
        PartyRequest::FetchAuditLog { limit } => {
            match state.audit_log(member, limit.clamp(1, 1000) as usize) {
                Ok(entries) => Dispatch::reply(PartyResponse::AuditLog(entries)),
                Err(e) => Dispatch::error(e),
            }
        }
        PartyRequest::FetchQuota => Dispatch::reply(PartyResponse::Quota(state.quota_for(member))),

        // --- Chunked upload -------------------------------------------------
        PartyRequest::StartUpload {
            name,
            mime,
            size,
            target,
        } => match state.start_upload(member, name, mime, size, target) {
            Ok(upload) => {
                conn.uploads.push(upload);
                Dispatch::reply(PartyResponse::UploadReady {
                    upload,
                    chunk_size: PARTY_CHUNK_BYTES as u32,
                })
            }
            Err(reason) => Dispatch::action_failed(upload_thread(member, target), reason),
        },
        PartyRequest::UploadChunk { upload, data } => {
            match state.upload_chunk(member, upload, &data) {
                Ok(()) => Dispatch::default(),
                Err(reason) => {
                    // A failed chunk ends the upload on *both* sides, or the
                    // spool would hold the member's slot until disconnect.
                    state.cancel_upload(member, upload);
                    conn.finish_upload(upload);
                    Dispatch::error(reason)
                }
            }
        }
        PartyRequest::FinishUpload { upload } => {
            // Phase 1 only; the write is deferred. The connection's list is
            // cleared only on failure: on success the id stays listed until
            // `connection.rs` finishes the deferred phase, because until then
            // it is a reservation a disconnect must release.
            match state.begin_upload(member, upload) {
                Ok(staged) => Dispatch::defer(Deferred::Upload(Box::new(staged))),
                Err(reason) => {
                    conn.finish_upload(upload);
                    Dispatch::action_failed(Uuid::nil(), reason)
                }
            }
        }
        PartyRequest::CancelUpload { upload } => {
            conn.finish_upload(upload);
            state.cancel_upload(member, upload);
            Dispatch::default()
        }
        // Re-sharing a file is an ordinary post that happens to reuse bytes the
        // server already holds, so it fans out like one.
        PartyRequest::ShareFile { hash, from, target } => {
            match state.share_file(member, &hash, from, target) {
                Ok(env) => posted(state, member, target, env),
                Err(reason) => Dispatch::action_failed(upload_thread(member, target), reason),
            }
        }
    }
}

fn create_channel(
    state: &mut PartyState,
    member: Uuid,
    name: &str,
    kind: ChannelKind,
    members: Vec<Uuid>,
) -> Dispatch {
    match state.create_channel_of_kind(member, name, kind, members) {
        Ok(_) => channel_list_refresh(state, member),
        Err(e) => Dispatch::error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::party::MessagePayload;

    /// Join and return the assigned member id.
    fn joined_id(state: &mut PartyState, conn: &mut ConnState, name: &str) -> Uuid {
        match &join(state, conn, name, None)[..] {
            [PartyResponse::Joined { member_id, .. }] => *member_id,
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    /// Join as the Owner, presenting the owner claim token as the password.
    fn joined_owner_id(state: &mut PartyState, conn: &mut ConnState, name: &str) -> Uuid {
        let token = state.owner_claim_token().expect("no owner yet").to_string();
        match &join(state, conn, name, Some(&token))[..] {
            [PartyResponse::Joined { member_id, .. }] => *member_id,
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    fn join(
        state: &mut PartyState,
        conn: &mut ConnState,
        name: &str,
        pw: Option<&str>,
    ) -> Vec<PartyResponse> {
        handle_request(
            state,
            conn,
            PartyRequest::Join {
                username: name.to_string(),
                password: pw.map(str::to_string),
            },
        )
        .replies
    }

    #[test]
    fn join_succeeds_and_sets_member() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let resp = join(&mut state, &mut conn, "alice", None);
        match &resp[..] {
            [PartyResponse::Joined {
                member_id,
                server_name,
                ..
            }] => {
                assert_eq!(server_name, "Srv");
                assert_eq!(conn.member(), Some(*member_id));
            }
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    /// One handshake must not buy unlimited password guesses: after a few
    /// rejections the connection is finished, forcing an attacker back through
    /// the RSA handshake (and the accept loop's per-IP limiter).
    #[test]
    fn repeated_wrong_passwords_exhaust_the_connection() {
        let mut state = PartyState::new("Srv", Some("pw".into()));
        let mut conn = ConnState::default();
        for _ in 0..MAX_JOIN_ATTEMPTS {
            let resp = join(&mut state, &mut conn, "alice", Some("nope"));
            assert!(matches!(resp[..], [PartyResponse::JoinRejected { .. }]));
        }
        // Past the cap the correct password is refused too — the connection
        // itself is spent, which is what makes reconnecting the only way on.
        let resp = join(&mut state, &mut conn, "alice", Some("pw"));
        match &resp[..] {
            [PartyResponse::JoinRejected { reason }] => {
                assert!(reason.contains("too many failed attempts"), "got: {reason}");
            }
            other => panic!("expected a rate-limited rejection, got {other:?}"),
        }
        assert_eq!(conn.member(), None);
    }

    /// Everyone already connected has to be told the directory changed, or
    /// their member list stays frozen at the moment they joined.
    #[test]
    fn joining_broadcasts_the_refreshed_directory() {
        let mut state = PartyState::new("Srv", None);
        join(&mut state, &mut ConnState::default(), "alice", None);

        let mut bob = ConnState::default();
        let out = handle_request(
            &mut state,
            &mut bob,
            PartyRequest::Join {
                username: "bob".into(),
                password: None,
            },
        );

        assert!(matches!(out.replies[..], [PartyResponse::Joined { .. }]));
        match &out.broadcast[..] {
            [PartyResponse::Members(members)] => {
                let names: Vec<&str> = members.iter().map(|m| m.username.as_str()).collect();
                assert_eq!(names, vec!["alice", "bob"]);
                assert!(members.iter().all(|m| m.online));
            }
            other => panic!("expected a Members broadcast, got {other:?}"),
        }
    }

    /// A DM must be delivered to the recipient *and* echoed to the sender's other
    /// devices (the runtime skips the originating connection).
    #[test]
    fn a_dm_is_directed_to_both_participants() {
        let mut state = PartyState::new("Srv", None);
        let mut alice_conn = ConnState::default();
        let alice = joined_id(&mut state, &mut alice_conn, "alice");
        let bob = joined_id(&mut state, &mut ConnState::default(), "bob");

        let out = handle_request(
            &mut state,
            &mut alice_conn,
            PartyRequest::SendDm {
                to: bob,
                text: "hello".into(),
            },
        );

        assert!(matches!(
            out.replies[..],
            [PartyResponse::MessagePosted { .. }]
        ));
        let targets: Vec<Uuid> = out.directed.iter().map(|(m, _)| *m).collect();
        assert_eq!(
            targets,
            vec![bob, alice],
            "the recipient and the sender's other devices both need this DM"
        );
        assert!(out
            .directed
            .iter()
            .all(|(_, r)| matches!(r, PartyResponse::Message(_))));
    }

    /// A private channel's messages must not be broadcast: that handed its live
    /// traffic to members correctly not shown the channel at all.
    #[test]
    fn a_private_channel_is_delivered_only_to_its_members() {
        let mut state = PartyState::new("Srv", None);
        let mut owner_conn = ConnState::default();
        let owner = joined_owner_id(&mut state, &mut owner_conn, "owner");
        let mut bob_conn = ConnState::default();
        let bob = joined_id(&mut state, &mut bob_conn, "bob");
        let mut carol_conn = ConnState::default();
        let carol = joined_id(&mut state, &mut carol_conn, "carol");

        let private = state
            .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![bob])
            .unwrap()
            .id;

        let out = handle_request(
            &mut state,
            &mut owner_conn,
            PartyRequest::PostMessage {
                channel: private,
                text: "members only".into(),
            },
        );

        assert!(
            out.broadcast.is_empty(),
            "a private channel must not be broadcast to every connection"
        );
        let targets: Vec<Uuid> = out.directed.iter().map(|(m, _)| *m).collect();
        assert!(targets.contains(&owner), "the poster's other devices");
        assert!(targets.contains(&bob), "a channel member");
        assert!(
            !targets.contains(&carol),
            "carol is not in the channel and must not receive its messages"
        );
    }

    /// A public channel still takes the cheap path.
    #[test]
    fn a_public_channel_is_still_broadcast() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let _owner = joined_id(&mut state, &mut conn, "owner");
        let channel = state.default_channel();
        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostMessage {
                channel,
                text: "hello all".into(),
            },
        );
        assert!(matches!(&out.broadcast[..], [PartyResponse::Message(_)]));
        assert!(out.directed.is_empty());
    }

    /// A DM addressed to yourself must not be delivered twice.
    #[test]
    fn a_self_dm_is_delivered_once() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let me = joined_id(&mut state, &mut conn, "alice");
        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::SendDm {
                to: me,
                text: "note to self".into(),
            },
        );
        assert_eq!(out.directed.len(), 1);
        assert_eq!(out.directed[0].0, me);
    }

    /// The cap counts *failures*, so an honest user who mistypes once and then
    /// gets it right is not punished.
    #[test]
    fn a_successful_join_is_unaffected_by_an_earlier_mistake() {
        let mut state = PartyState::new("Srv", Some("pw".into()));
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", Some("nope"));
        let resp = join(&mut state, &mut conn, "alice", Some("pw"));
        assert!(matches!(resp[..], [PartyResponse::Joined { .. }]));
        assert!(conn.member().is_some());
    }

    #[test]
    fn wrong_password_is_rejected_and_leaves_connection_unjoined() {
        let mut state = PartyState::new("Srv", Some("pw".into()));
        let mut conn = ConnState::default();
        let resp = join(&mut state, &mut conn, "alice", Some("nope"));
        assert!(matches!(resp[..], [PartyResponse::JoinRejected { .. }]));
        assert_eq!(conn.member(), None);
    }

    #[test]
    fn double_join_on_one_connection_errors() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);
        let resp = join(&mut state, &mut conn, "alice2", None);
        assert!(matches!(resp[..], [PartyResponse::Error(_)]));
    }

    #[test]
    fn post_then_fetch_history_round_trips_through_dispatch() {
        let mut state = PartyState::new("Srv", None);
        let channel = state.default_channel();
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);

        let posted = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostMessage {
                channel,
                text: "first message".into(),
            },
        );
        match &posted.replies[..] {
            [PartyResponse::MessagePosted { channel: c, seq }] => {
                assert_eq!(*c, channel);
                assert_eq!(*seq, 1);
            }
            other => panic!("expected MessagePosted, got {other:?}"),
        }
        // The post is also queued for broadcast to other members.
        assert!(
            matches!(&posted.broadcast[..], [PartyResponse::Message(env)] if env.seq == 1),
            "expected one broadcast Message with seq 1, got {:?}",
            posted.broadcast
        );

        let history = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::FetchHistory {
                channel,
                since_seq: 0,
            },
        )
        .replies;
        match &history[..] {
            [PartyResponse::History(items)] => {
                assert_eq!(items.len(), 1);
                assert_eq!(
                    items[0].payload,
                    MessagePayload::Text("first message".into())
                );
            }
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[test]
    fn list_members_and_channels_after_join() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);

        let members = handle_request(&mut state, &mut conn, PartyRequest::ListMembers).replies;
        assert!(matches!(&members[..], [PartyResponse::Members(m)] if m.len() == 1));

        let channels = handle_request(&mut state, &mut conn, PartyRequest::ListChannels).replies;
        assert!(matches!(&channels[..], [PartyResponse::Channels(c)] if c.len() == 1));
    }

    #[test]
    fn create_channel_replies_and_broadcasts_the_new_list() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);

        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::CreateChannel {
                name: "random".into(),
            },
        );
        // The creator gets their own filtered list; everyone else is nudged to
        // re-fetch theirs (see `channel_list_refresh`).
        assert!(matches!(&out.replies[..], [PartyResponse::Channels(c)] if c.len() == 2));
        assert!(matches!(
            &out.broadcast[..],
            [PartyResponse::DirectoryChanged]
        ));
    }

    /// Drive a dispatch to completion the way `connection.rs` does: perform any
    /// deferred I/O, then fold the resulting replies back in, so assertions
    /// cover end-to-end behaviour whichever half produced it.
    fn settle(state: &mut PartyState, member: Uuid, mut out: Dispatch) -> Dispatch {
        let Some(work) = out.deferred.take() else {
            return out;
        };
        let done = match work {
            Deferred::Download { hash, offset, plan } => {
                let total = plan.total();
                download_reply(hash, offset, total, plan.perform())
            }
            Deferred::Upload(staged) => {
                let mut staged = *staged;
                match staged.write_staging() {
                    Ok(()) => {
                        let committed = state.commit_upload(staged);
                        upload_committed(state, member, committed)
                    }
                    Err(reason) => {
                        state.discard_upload(staged);
                        Dispatch::action_failed(Uuid::nil(), reason)
                    }
                }
            }
        };
        out.replies.extend(done.replies);
        out.broadcast.extend(done.broadcast);
        out.directed.extend(done.directed);
        out
    }

    #[test]
    fn post_file_acks_the_poster_and_broadcasts_a_file_message() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let me = joined_id(&mut state, &mut conn, "alice");
        let channel = state.default_channel();
        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostFile {
                channel,
                name: "pic.png".into(),
                mime: "image/png".into(),
                data: b"\x89PNG data".to_vec(),
            },
        );
        let out = settle(&mut state, me, out);
        assert!(matches!(
            &out.replies[..],
            [PartyResponse::MessagePosted { .. }]
        ));
        match &out.broadcast[..] {
            [PartyResponse::Message(env)] => {
                assert!(matches!(&env.payload, MessagePayload::File(f) if f.name == "pic.png"));
            }
            other => panic!("expected a broadcast File message, got {other:?}"),
        }
    }

    /// SRV-14: `DownloadFile` returns the whole blob in one frame. The honest
    /// client uses it only up to `MAX_INLINE_FILE_BYTES`, but the server took
    /// it for any size: a member could make it read 100 MiB into memory per
    /// request, and the reply then broke the frame limit and the connection.
    /// Past the inline limit the answer is to use chunks, before a byte is read.
    #[test]
    fn a_whole_file_download_is_refused_past_the_inline_limit() {
        use messenger_core::party::{UploadTarget, PARTY_CHUNK_BYTES};
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);
        let me = conn.member().unwrap();
        let channel = state.default_channel();
        let payload = vec![7u8; MAX_INLINE_FILE_BYTES + 1];
        let upload = state
            .start_upload(
                me,
                "big.bin".into(),
                "application/octet-stream".into(),
                payload.len() as u64,
                UploadTarget::Channel(channel),
            )
            .unwrap();
        for chunk in payload.chunks(PARTY_CHUNK_BYTES) {
            state.upload_chunk(me, upload, chunk).unwrap();
        }
        state.finish_upload(me, upload).unwrap();
        let hash = messenger_core::party::blob_hash(&payload);

        let whole = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::DownloadFile { hash: hash.clone() },
        );
        assert!(
            whole.deferred.is_none(),
            "a read was planned for a whole 4 MiB+ blob"
        );
        assert!(
            matches!(&whole.replies[..], [PartyResponse::Error(e)] if e.contains("chunks")),
            "{:?}",
            whole.replies
        );
        // The chunked route still works.
        let chunk = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::DownloadChunk { hash, offset: 0 },
        );
        assert!(chunk.deferred.is_some());
    }

    #[test]
    fn download_returns_bytes_for_known_and_errors_for_unknown() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let me = joined_id(&mut state, &mut conn, "alice");
        let channel = state.default_channel();
        let post = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostFile {
                channel,
                name: "f.bin".into(),
                mime: "application/octet-stream".into(),
                data: b"payload".to_vec(),
            },
        );
        let post = settle(&mut state, me, post);
        let hash = match &post.broadcast[..] {
            [PartyResponse::Message(env)] => match &env.payload {
                MessagePayload::File(f) => f.hash.clone(),
                other => panic!("expected File payload, got {other:?}"),
            },
            other => panic!("expected broadcast, got {other:?}"),
        };

        // `handle_request` only authorises; the read is deferred to the runtime.
        let ok = handle_request(&mut state, &mut conn, PartyRequest::DownloadFile { hash });
        assert!(ok.replies.is_empty(), "the bytes must not be read inline");
        assert!(
            matches!(ok.deferred, Some(Deferred::Download { .. })),
            "expected a deferred download, got {:?}",
            ok.deferred
        );
        let reply = settle(&mut state, me, ok);
        assert!(
            matches!(&reply.replies[..], [PartyResponse::FileData { data, .. }] if data == b"payload")
        );

        // Denied and unknown are refused before anything is deferred.
        let missing = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::DownloadFile {
                hash: "deadbeef".to_string(),
            },
        );
        assert!(
            missing.deferred.is_none(),
            "nothing to read for an unknown file"
        );
        assert!(matches!(&missing.replies[..], [PartyResponse::Error(_)]));
    }

    /// SRV-06: posts are rate-limited per member; a refused post answers
    /// `ActionFailed` for its channel (so the client retracts the row) and costs
    /// other members nothing.
    #[test]
    fn writes_past_the_per_member_budget_are_refused_as_failed_sends() {
        let mut state = PartyState::new("Srv", None);
        let mut alice = ConnState::default();
        let mut bob = ConnState::default();
        let me = joined_id(&mut state, &mut alice, "alice");
        let bob_id = joined_id(&mut state, &mut bob, "bob");
        let channel = state.default_channel();
        let post = |text: &str| PartyRequest::PostMessage {
            channel,
            text: text.to_string(),
        };

        let mut refused = None;
        for i in 0..200 {
            let out = handle_request(&mut state, &mut alice, post(&format!("spam {i}")));
            if let [PartyResponse::ActionFailed { channel: c, reason }] = &out.replies[..] {
                refused = Some((i, *c, reason.clone()));
                break;
            }
        }
        let (at, c, reason) = refused.expect("a flood must be refused");
        // The burst, plus whatever trickled back in while the loop ran.
        let burst = crate::state::DEFAULT_WRITE_RATE.burst as usize;
        assert!(
            (burst..burst + 10).contains(&at),
            "refused after {at} posts"
        );
        assert_eq!(c, channel, "attributable to the channel the row is in");
        assert_eq!(reason, crate::state::WRITE_RATE_REFUSAL);

        // A refused DM is attributed to its thread, and governance gets an
        // ordinary error.
        let dm = handle_request(
            &mut state,
            &mut alice,
            PartyRequest::SendDm {
                to: bob_id,
                text: "hi".into(),
            },
        );
        assert!(matches!(
            &dm.replies[..],
            [PartyResponse::ActionFailed { channel: t, .. }]
                if *t == messenger_core::party::dm_thread_id(me, bob_id)
        ));
        let gov = handle_request(
            &mut state,
            &mut alice,
            PartyRequest::CreateChannel { name: "x".into() },
        );
        assert!(matches!(&gov.replies[..], [PartyResponse::Error(_)]));

        // Reads are never limited, and bob's budget is his own.
        let read = handle_request(&mut state, &mut alice, PartyRequest::ListMembers);
        assert!(matches!(&read.replies[..], [PartyResponse::Members(_)]));
        let out = handle_request(&mut state, &mut bob, post("still works"));
        assert!(matches!(
            &out.replies[..],
            [PartyResponse::MessagePosted { .. }]
        ));
    }
}
