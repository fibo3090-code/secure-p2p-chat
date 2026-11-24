// Rename / delete / info dialogs for a conversation.
import { useEffect, useState } from "react";
import { cx, Modal, Button, Input, Avatar } from "./ui.jsx";
import { toast } from "../lib/toast.js";
import { Icon } from "../lib/Icon.jsx";
import { SafetyGrid } from "./SafetyGrid.jsx";

export function RenameDialog({ target, onClose, onSubmit }) {
  const [name, setName] = useState("");
  // This component is always mounted (App renders it with target=null when
  // closed), so a `useState(target?.name)` initializer only ever ran once —
  // while target was still null. The field opened empty, and on the second
  // open it showed whatever was typed the previous time, for a different
  // conversation. Sync on every open instead.
  const targetId = target ? target.id : null;
  const targetName = target ? target.name || "" : "";
  useEffect(() => {
    if (targetId !== null) setName(targetName);
  }, [targetId, targetName]);
  if (!target) return null;
  return (
    <Modal open onClose={onClose} width={400} title="Rename conversation" icon="edit">
      <div className="creator-pane">
        <Input value={name} autoFocus onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && name.trim() && onSubmit(target.id, name.trim())} />
        <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button icon="check" disabled={!name.trim()} onClick={() => onSubmit(target.id, name.trim())}>Save</Button>
        </div>
      </div>
    </Modal>
  );
}

export function ConfirmDelete({ target, onClose, onConfirm }) {
  if (!target) return null;
  return (
    <Modal open onClose={onClose} width={400} title="Delete conversation" icon="trash">
      <div className="creator-pane">
        <p className="creator-lead">Delete <strong>{target.name}</strong> and its local history? This can't be undone. The peer is not notified.</p>
        <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
          <Button variant="ghost" onClick={onClose}>Cancel</Button>
          <Button variant="danger" icon="trash" onClick={() => onConfirm(target.id)}>Delete</Button>
        </div>
      </div>
    </Modal>
  );
}

const STATE_WORDS = { connected: "Connected", hosting: "Listening for a peer", offline: "Offline", connecting: "Connecting…" };

const TRANSPORT_LABEL = {
  direct: "Direct (peer-to-peer)",
  relay: "Via a relay — it can't read your messages, but it sees both IP addresses and when you talk",
  server: "Server (community)",
};

export function InfoDialog({ target, onClose, onRename, onDelete, myFingerprint }) {
  if (!target) return null;
  const fp = target.fingerprint || "";
  const transport = TRANSPORT_LABEL[target.transport] || TRANSPORT_LABEL.direct;
  const verified = target.trust === "verified" || target.trust === "trusted";
  const online = target.state === "connected";
  // One place that answers "is this really them?" — the state, what it means,
  // and when the check happens. The chip and the thread note both lead here,
  // so it must never be a bare grid of hex with no verdict.
  const trust = verified
    ? { cls: "is-ok", icon: "shieldCheck", h: "Verified",
      p: `You compared safety codes with ${target.name}. If their key ever changes, you'll be asked to check again before anything is sent.` }
    : online
      ? { cls: "is-pending", icon: "shieldAlert", h: "Not verified yet",
        p: "This connection hasn't been checked. The next time you connect, you'll both see a six-digit safety code — compare it by voice or in person before trusting it." }
      : { cls: "is-pending", icon: "shieldAlert", h: "Not verified yet",
        p: `You'll compare a safety code with ${target.name} the next time you connect. Until then, the encryption protects the messages, but nothing proves who is on the other end.` };
  return (
    <Modal open onClose={onClose} width={460} title="Conversation info" icon="info">
      <div className="info-body">
        <div className="info-head">
          <Avatar name={target.name} size={64} state={target.state} />
          <div className="info-name">{target.name}</div>
          <div className="info-state">{STATE_WORDS[target.state] || "Offline"}</div>
        </div>
        <div className={cx("trust-panel", trust.cls)}>
          <Icon name={trust.icon} size={18} />
          <div>
            <div className="trust-panel-h">{trust.h}</div>
            <p className="trust-panel-p">{trust.p}</p>
          </div>
        </div>
        <dl className="info-facts">
          <dt>Transport</dt><dd>{transport}</dd>
          <dt>Encryption</dt><dd>End-to-end · X25519 + AES-256-GCM · forward secrecy</dd>
        </dl>
        {!fp && <div className="verify-hint">No fingerprint yet.</div>}
        {fp && (
          // Side by side, so a later check is a real comparison: read your
          // fingerprint to them while they read theirs back, or hold the two
          // screens together. A lone grid of their key proved nothing.
          <details className="verify-advanced">
            <summary>Compare fingerprints</summary>
            <div className="fp-compare">
              <div className="fp-col">
                <span className="fp-col-h">{target.name}</span>
                <SafetyGrid fingerprint={fp} n={8} cell={18} />
                <code className="verify-code">{fp.replace(/(.{4})/g, "$1 ").trim()}</code>
              </div>
              {myFingerprint && (
                <div className="fp-col">
                  <span className="fp-col-h">You</span>
                  <SafetyGrid fingerprint={myFingerprint} n={8} cell={18} />
                  <code className="verify-code">{myFingerprint.replace(/(.{4})/g, "$1 ").trim()}</code>
                </div>
              )}
            </div>
            <p className="verify-hint">On their screen the two are swapped: their “You” must equal your “{target.name}”.</p>
          </details>
        )}
        <div className="info-actions" role="group" aria-label="Conversation actions">
          {fp && (
            <button className="info-action" onClick={() => navigator.clipboard?.writeText(fp)
              .then(() => toast("Fingerprint copied", "success"))
              .catch(() => toast("Could not copy", "error"))}>
              <Icon name="copy" size={16} /> Copy fingerprint
            </button>
          )}
          {onRename && !target.placeholder && (
            <button className="info-action" onClick={() => { onClose(); onRename(target); }}>
              <Icon name="edit" size={16} /> Rename
            </button>
          )}
          {onDelete && (
            <button className="info-action is-danger" onClick={() => { onClose(); onDelete(target); }}>
              <Icon name="trash" size={16} /> {target.placeholder ? "Stop listening" : "Delete conversation"}
            </button>
          )}
        </div>
      </div>
    </Modal>
  );
}
