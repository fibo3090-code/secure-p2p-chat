// How reachable you are, in one place. Listening used to be dressed up as a
// conversation ("Listening :12345" in the chat list, with a header, an info
// button and a menu), while the gate that refuses new peers was a separate
// switch at the bottom of the rail — two controls for one idea, and a socket
// impersonating a person. This is one status under the Chats title: what the
// state is, and a popover with the addresses to share and the two actions.
import { useEffect, useRef, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { api, friendlyError } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";
import { cx, Button } from "./ui.jsx";

/**
 * @param listeners   host placeholder conversations (each is a live listener)
 * @param locked      true when new peers are refused
 * @param connected   number of live sessions
 */
export function Reachability({ listeners = [], locked, connected = 0, onToggleLock, onStopListening, onChanged }) {
  const [open, setOpen] = useState(false);
  const [addr, setAddr] = useState(null);
  const [invite, setInvite] = useState("");
  const [busy, setBusy] = useState(false);
  const ref = useRef(null);
  const listening = listeners.length > 0;

  useEffect(() => {
    if (!open) return undefined;
    let live = true;
    api.myAddresses().then((a) => live && setAddr(a)).catch(() => {});
    api.myInviteLink().then((l) => live && setInvite(l || "")).catch(() => {});
    const onDown = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    const onKey = (e) => { if (e.key === "Escape") setOpen(false); };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => { live = false; window.removeEventListener("mousedown", onDown); window.removeEventListener("keydown", onKey); };
  }, [open]);

  const copy = (text, what) => navigator.clipboard?.writeText(text)
    .then(() => toast(`${what} copied`, "success"))
    .catch(() => toast("Could not copy", "error"));

  async function startListening() {
    setBusy(true);
    try {
      const s = await api.getSettings();
      await api.startHost(s?.listen_port || 12345, "");
      toast("Listening for peers", "success");
      onChanged && onChanged();
    } catch (e) { toast(friendlyError(e), "error"); }
    finally { setBusy(false); }
  }

  // One word for the state, one for the count. Neutral colours: being open to
  // new peers is the *more exposed* state, so it is never painted as "good".
  const state = locked ? "Closed to new peers" : listening ? "Listening" : "Not listening";
  const lines = [
    addr?.local && { label: "On this network", value: addr.local },
    addr?.external && { label: "Over the internet", value: addr.external },
  ].filter(Boolean);

  return (
    <div className="reach" ref={ref}>
      <button className={cx("reach-btn", locked && "is-closed", listening && !locked && "is-listening")}
        aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen((o) => !o)}
        aria-label={`${connected} connected. ${state}. Show how others can reach you.`}>
        {connected > 0 && <><span className="reach-dot is-live" aria-hidden="true" />{connected} connected<span className="reach-sep" aria-hidden="true">·</span></>}
        {locked ? <Icon name="ban" size={12} /> : listening ? <Icon name="radio" size={12} /> : null}
        <span>{state}</span>
        <Icon name="chevronDown" size={12} />
      </button>
      {open && (
        <div className="reach-pop" role="dialog" aria-label="Reachability">
          <div className="reach-pop-h">{listening ? "Others can reach you at" : "You're not listening"}</div>
          {listening ? (
            lines.length ? lines.map((l) => (
              <div key={l.label} className="reach-addr">
                <span className="reach-addr-l">{l.label}</span>
                <code className="reach-addr-v">{l.value}</code>
                <button className="reach-copy" onClick={() => copy(l.value, "Address")}
                  aria-label={`Copy ${l.label.toLowerCase()} address`}><Icon name="copy" size={13} /></button>
              </div>
            )) : <p className="reach-p">Looking up your address…</p>
          ) : (
            <p className="reach-p">Start listening so someone can dial you, or send them your invite link.</p>
          )}
          {invite && (
            <Button size="sm" variant="ghost" icon="link" onClick={() => copy(invite, "Invite link")}>Copy invite link</Button>
          )}
          <label className="reach-toggle">
            <span>
              <span className="reach-toggle-l">Accept new peers</span>
              <span className="reach-toggle-h">Off refuses anyone new. Existing conversations keep working.</span>
            </span>
            <input type="checkbox" role="switch" checked={!locked}
              onChange={() => onToggleLock && onToggleLock()} />
            <span className="reach-switch" aria-hidden="true" />
          </label>
          <div className="reach-actions">
            {listening
              ? <Button size="sm" variant="danger-ghost" icon="x" onClick={() => { setOpen(false); listeners.forEach((l) => onStopListening && onStopListening(l)); }}>Stop listening</Button>
              : <Button size="sm" icon="radio" disabled={busy} onClick={startListening}>Start listening</Button>}
          </div>
        </div>
      )}
    </div>
  );
}
