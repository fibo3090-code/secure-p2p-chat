// Rename / delete / info dialogs for a conversation.
import { useEffect, useState } from "react";
import { cx, Modal, Button, Input } from "./ui.jsx";
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

const TRANSPORT_LABEL = {
  direct: "direct (peer-to-peer)",
  relay: "relayed (blind broker, still end-to-end encrypted)",
  server: "server (community)",
};

export function InfoDialog({ target, onClose }) {
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
    <Modal open onClose={onClose} width={440} title={target.name} icon="info" sub="Conversation details">
      <div className="verify-body">
        <div className={cx("trust-panel", trust.cls)}>
          <Icon name={trust.icon} size={18} />
          <div>
            <div className="trust-panel-h">{trust.h}</div>
            <p className="trust-panel-p">{trust.p}</p>
          </div>
        </div>
        {!fp && <div className="verify-hint">No fingerprint yet.</div>}
        {fp && (
          <details className="verify-advanced">
            <summary>Full fingerprint (advanced)</summary>
            <SafetyGrid fingerprint={fp} n={8} cell={26} />
            <div className="verify-code">{fp.replace(/(.{4})/g, "$1 ").trim()}</div>
          </details>
        )}
        <div className="verify-hint">
          Transport: {transport}. End-to-end encrypted (X25519 + AES-256-GCM, forward secrecy).
        </div>
        <Button full variant="ghost" icon="x" onClick={onClose}>Close</Button>
      </div>
    </Modal>
  );
}
