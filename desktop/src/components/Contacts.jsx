// Contacts directory — saved peers (from invite links), with connect + import.
import { useCallback, useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, IconButton, Input, Modal, TrustBadge } from "./ui.jsx";
import { api } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";

export function Contacts({ onConnected }) {
  const [contacts, setContacts] = useState([]);
  const [link, setLink] = useState("");
  const [err, setErr] = useState("");
  // Deleting a contact throws away a verified fingerprint, which is the whole
  // basis of trusting that peer — re-establishing it means another out-of-band
  // check. Deleting a *conversation* already confirms, so doing it silently
  // here was both inconsistent and the more destructive of the two.
  const [deleteTarget, setDeleteTarget] = useState(null);

  const load = useCallback(() => {
    api.listContacts().then(setContacts).catch(() => {});
  }, []);
  useEffect(() => { load(); }, [load]);

  async function importLink() {
    setErr("");
    if (!link.trim()) return;
    try {
      const res = await api.importInvite(link.trim());
      setLink("");
      load();
      // An unsigned (v1) link carries no proof of who made it, so say so. The
      // contact is still Unverified either way and has to pass the safety code
      // on first connection — but importing both kinds with the same silent
      // success told the user nothing.
      if (res && res.signed === false) {
        toast("Added from an unsigned invite link — anyone can create one. Compare the safety code when you first connect.", "error");
      } else {
        toast(`Added ${res?.contact?.name || "contact"}.`, "success");
      }
    } catch (e) { setErr(String(e)); }
  }
  async function connect(id) {
    setErr("");
    try { await api.connectContact(id); onConnected && onConnected(); }
    catch (e) { setErr(String(e)); }
  }
  async function toggleBlock(c) {
    setErr("");
    try {
      if (c.trust === "blocked") await api.unblockContact(c.id);
      else await api.blockContact(c.id);
      load();
    } catch (e) { setErr(String(e)); }
  }
  // Say whether the copy happened. A clipboard write can be refused by the
  // webview, and silently doing nothing invites the user to paste a stale
  // fingerprint into a verification conversation.
  async function copyFingerprint(c) {
    if (!c.fingerprint) return;
    try {
      await navigator.clipboard?.writeText(c.fingerprint);
      toast("Fingerprint copied", "success");
    } catch {
      toast("Could not copy — select the fingerprint and copy it manually.", "error");
    }
  }
  async function remove(c) {
    setDeleteTarget(null);
    setErr("");
    try { await api.removeContact(c.id); load(); }
    catch (e) { setErr(String(e)); }
  }

  return (
    <div className="chat-pane">
      <header className="chat-head">
        <div className="chat-head-l">
          <div className="chat-head-info">
            <div className="chat-head-name">Contacts</div>
            <div className="chat-head-sub mono">{contacts.length} saved</div>
          </div>
        </div>
      </header>

      <div className="contacts-pane">
        <div className="creator-row contact-import">
          <Input value={link} placeholder="Paste an invite link to add a contact…"
            onChange={(e) => setLink(e.target.value)} onKeyDown={(e) => e.key === "Enter" && importLink()} />
          <Button icon="plus" onClick={importLink}>Add</Button>
        </div>
        {err && <div className="onb-err" style={{ marginTop: 12 }}><Icon name="alert" size={13} /> {err}</div>}

        <div className="contacts-grid contact-grid-tight">
          {contacts.length === 0 && (
            <div className="conv-empty">No contacts yet — add one with an invite link.</div>
          )}
          {contacts.map((c) => (
            <div key={c.id} className="contact-card">
              <div className="contact-card-top">
                <Avatar name={c.name} size={42} />
                <div className="contact-card-id">
                  <div className="contact-card-name">{c.name}</div>
                  <span className="contact-card-addr">{c.address || "no saved address"}</span>
                </div>
                <TrustBadge trust={c.trust} mini />
              </div>
              <div className="contact-card-fp">
                <span className="vf-grid-label">Fingerprint</span>
                <code>{c.fingerprint || "not verified yet"}</code>
              </div>
              <div className="contact-card-foot">
                <span className="contact-state">
                  <span className={cx("state-dot", c.reachable ? "state-offline" : "state-connecting")} />
                  {c.reachable ? (c.relay_only ? "ready · via relay" : "ready") : "no way to reach them"}
                </span>
                <div className="contact-card-actions">
                  {/* A contact imported from a relay invite has no direct
                      address but is still dialable through its relay token —
                      `reachable` covers both, `address` only covered one. */}
                  <IconButton name="send" label="Connect" onClick={() => connect(c.id)}
                    disabled={!c.reachable || c.trust === "blocked"} />
                  <IconButton name="copy" label="Copy fingerprint" disabled={!c.fingerprint}
                    onClick={() => copyFingerprint(c)} />
                  <IconButton name={c.trust === "blocked" ? "check" : "x"}
                    label={c.trust === "blocked" ? "Unblock" : "Block"} onClick={() => toggleBlock(c)} />
                  <IconButton name="trash" label="Delete contact" onClick={() => setDeleteTarget(c)} />
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>

      {deleteTarget && (
        <Modal open onClose={() => setDeleteTarget(null)} width={420}
          title="Delete contact" icon="trash">
          <div className="creator-pane">
            <p className="creator-lead">
              Delete <strong>{deleteTarget.name}</strong> from your contacts? This can't be undone.
            </p>
            {deleteTarget.fingerprint && (
              <p className="creator-lead">
                Their verified fingerprint is discarded too, so the next time they connect
                you would have to compare the safety code with them again out of band.
              </p>
            )}
            {deleteTarget.blocked && (
              <p className="creator-lead is-warn">
                <strong>This contact is blocked.</strong> The block is stored on the contact,
                so deleting it lets them connect to you again. Keep them blocked instead if
                that is what you want.
              </p>
            )}
            <p className="creator-lead">Your conversations and message history are kept.</p>
            <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
              <Button variant="ghost" onClick={() => setDeleteTarget(null)}>Cancel</Button>
              <Button variant="danger" icon="trash" onClick={() => remove(deleteTarget)}>Delete</Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  );
}
