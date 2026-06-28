// Phase-2 modals: a minimal "New connection" (Host / Connect) and the
// fingerprint-verification prompt. The full unified creator wizard is Phase 4.
import { useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { Modal, Button, Input } from "./ui.jsx";
import { api } from "../lib/bridge.js";

export function ConnectModal({ open, onClose }) {
  const [port, setPort] = useState("12345");
  const [addr, setAddr] = useState("");
  const [err, setErr] = useState("");

  async function host() {
    setErr("");
    try { await api.startHost(parseInt(port, 10) || 12345); onClose(); }
    catch (e) { setErr(String(e)); }
  }
  async function connect() {
    setErr("");
    const raw = addr.trim();
    if (!raw) return;
    const i = raw.lastIndexOf(":");
    const h = i > -1 ? raw.slice(0, i) : raw;
    const p = i > -1 ? parseInt(raw.slice(i + 1), 10) || 12345 : 12345;
    try { await api.connectPeer(h, p); onClose(); }
    catch (e) { setErr(String(e)); }
  }

  return (
    <Modal open={open} onClose={onClose} title="New connection" icon="plus"
      sub="Host a listener or dial a peer directly (Phase 2)">
      <div className="onb-field">
        <span className="onb-flabel">Host a listener on port</span>
        <div style={{ display: "flex", gap: 8 }}>
          <Input value={port} onChange={(e) => setPort(e.target.value)} style={{ maxWidth: 120 }} />
          <Button variant="ghost" icon="server" onClick={host}>Start host</Button>
        </div>
      </div>
      <div className="onb-field" style={{ marginTop: 14 }}>
        <span className="onb-flabel">Connect to a peer (host:port)</span>
        <div style={{ display: "flex", gap: 8 }}>
          <Input value={addr} placeholder="192.168.1.20:12345" onChange={(e) => setAddr(e.target.value)} />
          <Button icon="send" onClick={connect}>Connect</Button>
        </div>
      </div>
      {err && <div className="onb-err" style={{ marginTop: 12 }}><Icon name="alert" size={13} /> {err}</div>}
    </Modal>
  );
}

export function FingerprintModal({ req, onClose }) {
  if (!req) return null;
  async function decide(accept) {
    try { await api.confirmFingerprint(req.chat_id, accept); } catch (e) { /* ignore */ }
    onClose();
  }
  return (
    <Modal open={!!req} onClose={onClose} title={`Verify ${req.peer_name}`} icon="fingerprint"
      sub="Compare this fingerprint out of band before trusting the peer">
      <code className="onb-fp-code mono" style={{ display: "block", wordBreak: "break-all", marginBottom: 16 }}>
        {req.fingerprint}
      </code>
      <div style={{ display: "flex", gap: 10 }}>
        <Button icon="shieldCheck" onClick={() => decide(true)}>Verify &amp; trust</Button>
        <Button variant="danger-ghost" icon="x" onClick={() => decide(false)}>Reject</Button>
      </div>
    </Modal>
  );
}
