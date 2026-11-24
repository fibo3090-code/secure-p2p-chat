// Contacts directory — saved peers (from invite links), with connect + import.
import { useCallback, useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, Modal, Page, OverflowMenu } from "./ui.jsx";
import { api, friendlyError } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";

export function Contacts({ onConnected, onOpenChat, connectedChats }) {
  const [contacts, setContacts] = useState([]);
  const [link, setLink] = useState("");
  const [err, setErr] = useState("");
  // Deleting a contact throws away a verified fingerprint, which is the whole
  // basis of trusting that peer — re-establishing it means another out-of-band
  // check. Deleting a *conversation* already confirms, so doing it silently
  // here was both inconsistent and the more destructive of the two.
  const [deleteTarget, setDeleteTarget] = useState(null);

  const load = useCallback(() => {
    // `|| []` because the whole pane renders off `contacts.length`: one command
    // answering with nothing takes the surface down entirely, and a contacts
    // list is not worth a blank screen.
    api.listContacts().then((list) => setContacts(list || [])).catch(() => {});
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
    } catch (e) { setErr(friendlyError(e)); }
  }
  async function connect(id) {
    setErr("");
    try { await api.connectContact(id); onConnected && onConnected(); }
    catch (e) { setErr(friendlyError(e)); }
  }
  async function toggleBlock(c) {
    setErr("");
    try {
      if (c.trust === "blocked") await api.unblockContact(c.id);
      else await api.blockContact(c.id);
      load();
    } catch (e) { setErr(friendlyError(e)); }
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
    catch (e) { setErr(friendlyError(e)); }
  }

  const verifiedCount = contacts.filter((c) => c.trust === "verified" || c.trust === "trusted").length;
  return (
    <Page title="Contacts"
      sub={contacts.length === 0 ? "People you add from an invite link"
        : `${contacts.length} saved · ${verifiedCount} verified`}>
      <form className="contact-import" onSubmit={(e) => { e.preventDefault(); void importLink(); }}>
        <label className="field-label" htmlFor="invite-link">Add someone from their invite link</label>
        <div className="creator-row">
          <input id="invite-link" className="input" value={link}
            placeholder="Paste an invite link (chat-p2p://invite/…)"
            onChange={(e) => setLink(e.target.value)} />
          <Button icon="userPlus" type="submit">Add</Button>
        </div>
        <p className="field-hint">
          A link tells the app how to reach someone. It is not proof of who they are — you
          still compare the safety code the first time you connect.
        </p>
        {err && <div className="onb-err"><Icon name="alert" size={13} /> {err}</div>}
      </form>

      <ul className="list-card" aria-label="Contacts">
        {contacts.length === 0 && (
          <li className="list-empty">
            <Icon name="user" size={22} />
            <span>No contacts yet. Paste an invite link above to add your first one.</span>
          </li>
        )}
        {contacts.map((c) => {
          const blocked = c.trust === "blocked";
          const verified = c.trust === "verified" || c.trust === "trusted";
          // How you would reach them, in words. "No way to reach them", in
          // Title Case next to an amber dot, read like a fault in the app.
          // Already talking: offer the conversation, not a second session.
          const live = !!(c.chat_id && connectedChats && connectedChats.has(c.chat_id));
          const reach = blocked ? "Blocked" : live ? "Connected"
            : !c.reachable ? "No saved address"
              : c.relay_only ? "Reachable via relay" : "Reachable directly";
          return (
            <li key={c.id} className={cx("contact-row", blocked && "is-blocked")}>
              <Avatar name={c.name} size={42} />
              {/* The row opens the conversation you already have with them;
                  Connect is the separate, explicit action that dials. */}
              <button className="contact-row-main" disabled={!c.chat_id || !onOpenChat}
                title={c.chat_id ? `Open your conversation with ${c.name}` : undefined}
                onClick={() => c.chat_id && onOpenChat && onOpenChat(c.chat_id)}>
                <div className="contact-row-l1">
                  <span className="contact-row-name">{c.name}</span>
                  <span className={cx("trust-word", blocked ? "is-blocked" : verified ? "is-ok" : "is-warn")}>
                    <Icon name={blocked ? "ban" : verified ? "shieldCheck" : "shieldAlert"} size={13} />
                    {blocked ? "Blocked" : verified ? "Verified" : "Not verified"}
                  </span>
                </div>
                <div className="contact-row-l2">
                  <span>{c.address || reach}</span>
                  {c.address && <><span className="chat-dot" aria-hidden="true">·</span><span>{reach}</span></>}
                </div>
              </button>
              <div className="contact-row-actions">
                {/* A contact imported from a relay invite has no direct
                    address but is still dialable through its relay token —
                    `reachable` covers both, `address` only covered one. */}
                {live ? (
                  <Button size="sm" variant="ghost" icon="message" onClick={() => onOpenChat && onOpenChat(c.chat_id)}>
                    Message
                  </Button>
                ) : (
                  <Button size="sm" variant="ghost" icon="plug"
                    title={c.reachable ? `Connect to ${c.name}` : "No saved address for this contact"}
                    onClick={() => connect(c.id)} disabled={!c.reachable || blocked}>
                    Connect
                  </Button>
                )}
                <OverflowMenu label={`More actions for ${c.name}`} items={[
                  { icon: "copy", label: "Copy fingerprint", disabled: !c.fingerprint, onClick: () => copyFingerprint(c) },
                  { icon: blocked ? "check" : "ban", label: blocked ? "Unblock" : "Block", onClick: () => toggleBlock(c) },
                  "sep",
                  { icon: "trash", label: "Delete contact", danger: true, onClick: () => setDeleteTarget(c) },
                ]} />
              </div>
            </li>
          );
        })}
      </ul>

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
    </Page>
  );
}
