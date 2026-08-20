// @vitest-environment jsdom
//
// The shared primitives. `Modal` carries most of the weight here: it is the only
// place the app implements a focus trap and the "this decision cannot be waved
// away" rule for the TOFU prompt, and neither is visible from reading a snapshot.

import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  cx,
  Button,
  IconButton,
  Avatar,
  TrustBadge,
  TransportBadge,
  Input,
  PasswordInput,
  Modal,
} from "./ui.jsx";

describe("cx", () => {
  it("joins the truthy class names and drops the rest", () => {
    expect(cx("a", false, "b", null, undefined, "c")).toBe("a b c");
    expect(cx()).toBe("");
  });
});

describe("Button", () => {
  it("passes through disabled and click handlers", async () => {
    const onClick = vi.fn();
    const { rerender } = render(<Button onClick={onClick}>Send</Button>);
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onClick).toHaveBeenCalledTimes(1);

    rerender(<Button onClick={onClick} disabled>Send</Button>);
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

describe("IconButton", () => {
  it("has an accessible name — it renders no text of its own", () => {
    render(<IconButton name="fingerprint" label="Verify fingerprint" />);
    expect(screen.getByRole("button", { name: "Verify fingerprint" })).toBeTruthy();
  });
});

describe("Avatar", () => {
  it("shows initials and is stable for a given name", () => {
    const { container, rerender } = render(<Avatar name="Ada Lovelace" />);
    expect(container.textContent).toContain("AL");
    const first = container.querySelector(".avatar").getAttribute("style");
    rerender(<Avatar name="Ada Lovelace" />);
    expect(container.querySelector(".avatar").getAttribute("style")).toBe(first);
  });

  it("survives an empty name rather than throwing", () => {
    const { container } = render(<Avatar name="" />);
    expect(container.querySelector(".avatar")).toBeTruthy();
  });
});

describe("TrustBadge", () => {
  it("names the trust state, and treats an unknown one as unverified", () => {
    const { rerender, container } = render(<TrustBadge trust="verified" />);
    expect(container.textContent).toContain("Verified");
    rerender(<TrustBadge trust="something-new" />);
    expect(container.textContent).toContain("Unverified");
  });
});

describe("TransportBadge", () => {
  it("labels the transport and defaults to direct", () => {
    const { rerender, container } = render(<TransportBadge transport="relay" />);
    expect(container.textContent).toContain("Via relay");
    rerender(<TransportBadge transport={undefined} />);
    expect(container.textContent).toContain("Direct P2P");
  });
});

describe("Input / PasswordInput", () => {
  it("masks the password until the toggle is used", async () => {
    render(<PasswordInput value="hunter2hunter2" onChange={() => {}} />);
    const field = document.querySelector("input");
    expect(field.type).toBe("password");
    await userEvent.click(screen.getByRole("button", { name: "Toggle visibility" }));
    expect(field.type).toBe("text");
  });

  it("tells the password manager which field this is", async () => {
    // Wrong values are actively harmful: "current-password" on a new-password
    // field invites the manager to fill the old one, and "new-password" on an
    // unlock field offers to invent one for someone trying to get back in.
    const { rerender } = render(<PasswordInput value="" onChange={() => {}} autoComplete="new-password" />);
    expect(document.querySelector("input").getAttribute("autocomplete")).toBe("new-password");

    rerender(<PasswordInput value="" onChange={() => {}} autoComplete="current-password" />);
    expect(document.querySelector("input").getAttribute("autocomplete")).toBe("current-password");
  });

  it("defaults to off rather than guessing", () => {
    // A connection or community password is not the identity password, and a
    // manager should not offer one for the other.
    render(<PasswordInput value="" onChange={() => {}} />);
    expect(document.querySelector("input").getAttribute("autocomplete")).toBe("off");
  });

  it("forwards arbitrary props to the underlying input", () => {
    render(<Input placeholder="Relay address" aria-label="Relay address" />);
    expect(screen.getByLabelText("Relay address")).toBeTruthy();
  });
});

describe("Modal", () => {
  it("is a labelled dialog and focuses itself, not its first control", () => {
    render(
      <Modal open onClose={() => {}} title="Rename conversation" sub="Details">
        <button>Cancel</button>
        <button>Save</button>
      </Modal>,
    );
    const dialog = screen.getByRole("dialog", { name: "Rename conversation" });
    // A confirmation dialog must not open with a button already focused, one
    // Space away from firing.
    expect(document.activeElement).toBe(dialog);
  });

  it("closes on Escape and on a scrim click when dismissable", async () => {
    const onClose = vi.fn();
    const { container } = render(
      <Modal open onClose={onClose} title="Info"><button>OK</button></Modal>,
    );
    await userEvent.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);

    await userEvent.click(container.querySelector(".modal-scrim"));
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it("refuses Escape, the scrim and the close button when not dismissable", async () => {
    // This is the TOFU prompt's contract: a live session is waiting on the
    // answer, and dismissing it left the peer hanging with no explanation.
    const onClose = vi.fn();
    const { container } = render(
      <Modal open onClose={onClose} title="Verify Alice" dismissable={false}>
        <button>Trust</button>
      </Modal>,
    );
    await userEvent.keyboard("{Escape}");
    await userEvent.click(container.querySelector(".modal-scrim"));
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "Close" })).toBeNull();
  });

  it("keeps Tab inside the dialog", async () => {
    render(
      <Modal open onClose={() => {}} title="Trap">
        <button>First</button>
        <button>Last</button>
      </Modal>,
    );
    const dialog = screen.getByRole("dialog");
    const first = within(dialog).getByRole("button", { name: "First" });
    const last = within(dialog).getByRole("button", { name: "Last" });

    // jsdom performs no layout, so `offsetParent` is null for everything and the
    // trap's own visibility filter would discard both buttons. Report them as
    // laid out so the test exercises the real wrap-around rather than the
    // degenerate one-item case.
    for (const el of [first, last]) {
      Object.defineProperty(el, "offsetParent", { get: () => dialog });
    }

    last.focus();
    await userEvent.tab();
    // Without the trap this walks into the app behind the scrim, where clicks
    // are blocked but focus is not.
    expect(document.activeElement).toBe(first);

    first.focus();
    await userEvent.tab({ shift: true });
    expect(document.activeElement).toBe(last);
  });

  it("renders nothing when closed", () => {
    render(<Modal open={false} onClose={() => {}} title="Hidden"><span>body</span></Modal>);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
