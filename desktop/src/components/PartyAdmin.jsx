// Community governance surfaces: the Drive panel (shared files + quota), the
// audit log, and the channel-access dialog.
//
// Everything here is advisory. The server decides who may do what and refuses
// anything it disagrees with; hiding a control the caller cannot use is about
// not offering a button that will be rejected, never about enforcement.
import { useMemo, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Button, Input, Modal } from "./ui.jsx";

export const ROLES = ["guest", "member", "admin"];

export const KINDS = [
  { id: "public", label: "Public", hint: "Everyone who joined can read and post." },
  { id: "private", label: "Private", hint: "Only the members you pick can see it at all." },
  { id: "locked", label: "Locked", hint: "Frozen: everyone can read the history, nobody but an admin can add to it." },
  { id: "announce", label: "Announce", hint: "An announcement feed: everyone can read, only admins can post." },
];

const KIND_ICON = { public: "hash", private: "lock", locked: "lock", announce: "megaphone" };

/// Icon name for a channel kind, falling back to the plain hash.
export function kindIcon(kind) {
  return KIND_ICON[kind] || "hash";
}

export function fmtBytes(n) {
  if (n == null) return "";
  const u = ["B", "KB", "MB", "GB"];
  let i = 0, v = Number(n);
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v < 10 && i > 0 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
}

function fmtDate(ms) {
  if (!ms) return "";
  const d = new Date(Number(ms));
  return d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/// The Drive: every file the member may see, with who shared it and where.
export function DrivePanel({ server, onDownload, onDelete, onShare, onPermissions, downloading }) {
  const [confirm, setConfirm] = useState(null); // `${hash}|${location}` armed for delete
  const files = server?.files || [];
  const quota = server?.quota;

  // Quota is reported in distinct bytes: sharing one file into three channels
  // costs its size once, which is also what freeing it requires undoing.
  const pct = quota?.limit ? Math.min(100, (quota.used / quota.limit) * 100) : null;

  return (
    <div className="drive-pane">
      <div className="drive-head">
        <div>
          <div className="drive-title">Shared files</div>
          <div className="drive-sub">
            {files.length === 0
              ? "Nothing has been shared here yet."
              : `${files.length} file${files.length === 1 ? "" : "s"} you can see.`}
          </div>
        </div>
        {quota && (
          <div className="drive-quota" title={
            `You are using ${fmtBytes(quota.used)}${quota.limit ? ` of ${fmtBytes(quota.limit)}` : " (no personal limit)"}. `
            + `This server holds ${fmtBytes(quota.server_used)} of ${fmtBytes(quota.server_limit)}.`
          }>
            <div className="drive-quota-l">
              {quota.limit
                ? <>{fmtBytes(quota.used)} of {fmtBytes(quota.limit)} used</>
                : <>{fmtBytes(quota.used)} used · no personal limit</>}
            </div>
            {pct != null && (
              <div className="drive-bar"><span style={{ width: `${pct}%` }} /></div>
            )}
            <div className="drive-quota-s">
              Server: {fmtBytes(quota.server_used)} / {fmtBytes(quota.server_limit)}
            </div>
          </div>
        )}
      </div>

      {files.length > 0 && (
        <div className="drive-list">
          {files.map((f) => {
            const key = `${f.hash}|${f.location}`;
            const armed = confirm === key;
            return (
              <div className="drive-row" key={key}>
                <span className="drive-ic"><Icon name="file" size={16} /></span>
                <div className="drive-meta">
                  <div className="drive-name">{f.name}</div>
                  <div className="drive-sub2">
                    {fmtBytes(f.size)}
                    <span className="chat-dot">·</span>
                    {f.uploader_name}
                    <span className="chat-dot">·</span>
                    {f.location_name}
                    <span className="chat-dot">·</span>
                    {fmtDate(f.shared_at)}
                  </div>
                </div>
                {f.can_download && (
                  <button className="drive-act" title={`Download ${f.name}`}
                    disabled={!!downloading?.[f.hash]}
                    onClick={() => onDownload(f)}>
                    <Icon name={downloading?.[f.hash] ? "clock" : "download"} size={15} />
                  </button>
                )}
                {f.can_share && (
                  <button className="drive-act"
                    title={`Share ${f.name} somewhere else — the file is stored once, so this costs nothing`}
                    onClick={() => onShare(f)}>
                    <Icon name="swap" size={15} />
                  </button>
                )}
                {f.can_delete && (
                  <button className="drive-act"
                    title={`Choose who can use ${f.name}`}
                    onClick={() => onPermissions(f)}>
                    <Icon name="key" size={15} />
                  </button>
                )}
                {f.can_delete && (
                  <button className={cx("drive-act", "drive-del", armed && "is-confirm")}
                    title={armed
                      ? "Click again to delete. The file stays listed in the conversation but can no longer be downloaded."
                      : "Delete this file"}
                    onBlur={() => setConfirm(null)}
                    onClick={() => {
                      if (!armed) { setConfirm(key); return; }
                      setConfirm(null);
                      onDelete(f);
                    }}>
                    <Icon name="trash" size={15} />
                  </button>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

/// The audit log: who did what, newest first. Admins only — the server refuses
/// the request for anyone else, so this pane is not offered to them.
export function AuditPanel({ server }) {
  const entries = server?.audit || [];
  return (
    <div className="drive-pane">
      <div className="drive-head">
        <div>
          <div className="drive-title">Activity log</div>
          <div className="drive-sub">
            {entries.length === 0
              ? "No administrative actions have been recorded yet."
              : "Role changes, channel changes and file deletions on this server."}
          </div>
        </div>
      </div>
      {entries.length > 0 && (
        <div className="drive-list">
          {entries.map((e, i) => (
            <div className="audit-row" key={i}>
              <span className="audit-when mono">{fmtDate(e.at)}</span>
              <span className="audit-who">{e.actor_name}</span>
              <span className="audit-what">{e.detail}</span>
              <span className="audit-tag mono">{e.action}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/// Create a channel, or change an existing one's access. `channel` is null when
/// creating. Private membership is only asked for when it is what decides access.
export function ChannelAccessDialog({ server, channel, onClose, onSubmit }) {
  const editing = !!channel;
  const [name, setName] = useState(channel?.name || "");
  const [kind, setKind] = useState(channel?.kind || "public");
  const [picked, setPicked] = useState(() => new Set(channel?.members || []));
  const [busy, setBusy] = useState(false);

  const others = useMemo(
    () => (server?.members || []).filter((m) => !m.is_me),
    [server],
  );
  const hint = KINDS.find((k) => k.id === kind)?.hint || "";

  function toggle(id) {
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }

  async function submit() {
    if (!editing && !name.trim()) return;
    setBusy(true);
    try {
      await onSubmit({ name: name.trim(), kind, members: [...picked] });
      onClose();
    } finally { setBusy(false); }
  }

  return (
    <Modal open onClose={onClose} width={480}
      icon={kindIcon(kind)}
      title={editing ? `Access for #${channel.name}` : "New channel"}
      sub={editing
        ? "Changing this takes effect immediately for everyone."
        : "Channels keep their history on the server."}>
      <div className="creator-pane">
      {!editing && (
        <label className="fld">
          <span className="fld-l">Name</span>
          <Input value={name} maxLength={64} autoFocus placeholder="announcements"
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submit()} />
        </label>
      )}

      <div className="fld">
        <span className="fld-l">Who can use it</span>
        <div className="kind-grid">
          {KINDS.map((k) => (
            <button key={k.id} type="button"
              className={cx("kind-opt", kind === k.id && "is-active")}
              aria-pressed={kind === k.id}
              onClick={() => setKind(k.id)}>
              <Icon name={kindIcon(k.id)} size={14} />
              <span>{k.label}</span>
            </button>
          ))}
        </div>
        <div className="fld-hint">{hint}</div>
      </div>

      {kind === "private" && (
        <div className="fld">
          <span className="fld-l">Members</span>
          <div className="member-pick">
            {others.length === 0 && <div className="fld-hint">Nobody else has joined yet.</div>}
            {others.map((m) => (
              <button key={m.id} type="button"
                className={cx("pick-opt", picked.has(m.id) && "is-active")}
                aria-pressed={picked.has(m.id)}
                onClick={() => toggle(m.id)}>
                <Icon name={picked.has(m.id) ? "check" : "user"} size={13} /> {m.username}
              </button>
            ))}
          </div>
          <div className="fld-hint">
            You are always included, so you cannot lock yourself out of a channel you made.
            Admins can see private channels in order to moderate them.
          </div>
        </div>
      )}

      <div className="modal-actions">
        <Button variant="ghost" onClick={onClose} disabled={busy}>Cancel</Button>
        <Button icon="check" onClick={submit} disabled={busy || (!editing && !name.trim())}>
          {editing ? "Save" : "Create channel"}
        </Button>
      </div>
      </div>
    </Modal>
  );
}
/// Choose what a shared file grants, either to everyone who can reach it or to
/// one member. The server refuses anything the caller does not hold themselves,
/// so this is about not offering a switch that will be rejected.
export function FilePermissionsDialog({ server, file, onClose, onSubmit }) {
  const [scope, setScope] = useState("default"); // "default" | member id
  const [perms, setPerms] = useState({
    view: file.can_view !== false,
    download: file.can_download !== false,
    delete: false,
    share: false,
  });
  const [busy, setBusy] = useState(false);

  const others = useMemo(
    () => (server?.members || []).filter((m) => !m.is_me),
    [server],
  );

  // Downloading what you cannot see, and sharing what you cannot download, are
  // not coherent — mirror the server's normalisation so the switches behave.
  function toggle(key) {
    setPerms((p) => {
      const next = { ...p, [key]: !p[key] };
      if (next.share) next.download = true;
      if (next.download || next.delete || next.share) next.view = true;
      if (!next.view) { next.download = false; next.delete = false; next.share = false; }
      if (!next.download) next.share = false;
      return next;
    });
  }

  async function submit() {
    setBusy(true);
    try {
      await onSubmit(scope === "default" ? null : scope, perms);
      onClose();
    } finally { setBusy(false); }
  }

  const RIGHTS = [
    ["view", "See it", "It appears in their file list and in the conversation."],
    ["download", "Download it", "They can save a copy of the actual file."],
    ["delete", "Remove it", "They can take this share down."],
    ["share", "Share it on", "They can post it in other channels or DMs."],
  ];

  return (
    <Modal open onClose={onClose} width={470} icon="key"
      title={`Who can use ${file.name}`}
      sub="Applies to this share only — the same file elsewhere keeps its own settings.">
      <div className="creator-pane">
        <div className="fld">
          <span className="fld-l">Applies to</span>
          <div className="member-pick">
            <button type="button"
              className={cx("pick-opt", scope === "default" && "is-active")}
              aria-pressed={scope === "default"}
              onClick={() => setScope("default")}>
              <Icon name="users" size={13} /> Everyone here
            </button>
            {others.map((m) => (
              <button key={m.id} type="button"
                className={cx("pick-opt", scope === m.id && "is-active")}
                aria-pressed={scope === m.id}
                onClick={() => setScope(m.id)}>
                <Icon name="user" size={13} /> {m.username}
              </button>
            ))}
          </div>
        </div>

        <div className="fld">
          <span className="fld-l">They can</span>
          <div className="perm-list">
            {RIGHTS.map(([key, label, hint]) => (
              <button key={key} type="button"
                className={cx("perm-opt", perms[key] && "is-active")}
                aria-pressed={!!perms[key]}
                onClick={() => toggle(key)}>
                <Icon name={perms[key] ? "check" : "x"} size={14} />
                <span className="perm-txt">
                  <span className="perm-l">{label}</span>
                  <span className="perm-h">{hint}</span>
                </span>
              </button>
            ))}
          </div>
          <div className="fld-hint">
            You can only pass on what you hold yourself, and you always keep full
            control of a file you shared.
          </div>
        </div>

        <div className="modal-actions">
          <Button variant="ghost" onClick={onClose} disabled={busy}>Cancel</Button>
          <Button icon="check" onClick={submit} disabled={busy}>Save</Button>
        </div>
      </div>
    </Modal>
  );
}

/// Pick a channel or member to re-share a file into.
export function ShareFileDialog({ server, file, onClose, onSubmit }) {
  const [busy, setBusy] = useState(false);
  // Somewhere it already is, is not a destination.
  const channels = (server?.channels || []).filter(
    (c) => c.can_post !== false && c.id !== file.location,
  );
  const members = (server?.members || []).filter((m) => !m.is_me);

  async function pick(dest) {
    setBusy(true);
    try { await onSubmit(dest); onClose(); } finally { setBusy(false); }
  }

  return (
    <Modal open onClose={onClose} width={430} icon="swap"
      title={`Share ${file.name}`}
      sub="It is stored once, so this does not upload it again.">
      <div className="creator-pane">
        <div className="fld">
          <span className="fld-l">Channels</span>
          <div className="member-pick">
            {channels.length === 0 && <div className="fld-hint">Nowhere else to post it.</div>}
            {channels.map((c) => (
              <button key={c.id} type="button" className="pick-opt" disabled={busy}
                onClick={() => pick({ channel: c.id })}>
                <Icon name={kindIcon(c.kind)} size={13} /> {c.name}
              </button>
            ))}
          </div>
        </div>
        <div className="fld">
          <span className="fld-l">Direct message</span>
          <div className="member-pick">
            {members.length === 0 && <div className="fld-hint">Nobody else has joined yet.</div>}
            {members.map((m) => (
              <button key={m.id} type="button" className="pick-opt" disabled={busy}
                onClick={() => pick({ peer: m.id })}>
                <Icon name="user" size={13} /> {m.username}
              </button>
            ))}
          </div>
        </div>
        <div className="modal-actions">
          <Button variant="ghost" onClick={onClose} disabled={busy}>Cancel</Button>
        </div>
      </div>
    </Modal>
  );
}
