// Rename / delete / info dialogs for a conversation.
import { useEffect, useState } from "react";
import { Modal, Button, Input } from "./ui.jsx";
import { SafetyGrid } from "./SafetyGrid.jsx";

export function RenameDialog({ target, onClose, onSubmit }) {
  const [name, setName] = useState("");
  // This component is always mounted (App renders it with target=null when
  // closed), so a `useState(target?.name)` initializer only ever ran once —
  // while target was still null. The field opened empty, and on the second
  // open it showed whatever was typed the previous time, for a different
  // conversation. Sync on every open instead.
  useEffect(() => {
    if (target) setName(target.name || "");
  }, [target && target.id, target && target.name]);
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
  return (
    <Modal open onClose={onClose} width={420} title={target.name} icon="info" sub="Conversation details">
      <div className="verify-body">
        {fp ? <SafetyGrid fingerprint={fp} n={8} cell={26} /> : <div className="verify-hint">No fingerprint yet.</div>}
        {fp && <div className="verify-code">{fp.replace(/(.{4})/g, "$1 ").trim()}</div>}
        <div className="verify-hint">
          Transport: {transport}. End-to-end encrypted with X25519 + AES-256-GCM.
        </div>
        <Button full variant="ghost" icon="x" onClick={onClose}>Close</Button>
      </div>
    </Modal>
  );
}
