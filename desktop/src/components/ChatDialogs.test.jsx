// @vitest-environment jsdom
//
// The rename / delete / info dialogs. `RenameDialog` is the interesting one: it
// stays mounted with `target=null`, so a `useState(target?.name)` initialiser
// ran exactly once — while target was still null — and the field then showed the
// *previous* conversation's typed name on the next open.

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RenameDialog, ConfirmDelete, InfoDialog } from "./ChatDialogs.jsx";

const chat = { id: "chat-1", name: "Alice", fingerprint: "ab".repeat(32), transport: "direct" };

describe("RenameDialog", () => {
  it("renders nothing without a target", () => {
    render(<RenameDialog target={null} onClose={() => {}} onSubmit={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens pre-filled with the current name", () => {
    render(<RenameDialog target={chat} onClose={() => {}} onSubmit={() => {}} />);
    expect(document.querySelector("input").value).toBe("Alice");
  });

  it("re-syncs when a different conversation is opened", async () => {
    const { rerender } = render(<RenameDialog target={chat} onClose={() => {}} onSubmit={() => {}} />);
    await userEvent.clear(document.querySelector("input"));
    await userEvent.type(document.querySelector("input"), "typed but never saved");

    rerender(<RenameDialog target={null} onClose={() => {}} onSubmit={() => {}} />);
    rerender(<RenameDialog target={{ id: "chat-2", name: "Bob" }} onClose={() => {}} onSubmit={() => {}} />);
    expect(document.querySelector("input").value).toBe("Bob");
  });

  it("submits the trimmed name, and refuses an empty one", async () => {
    const onSubmit = vi.fn();
    render(<RenameDialog target={chat} onClose={() => {}} onSubmit={onSubmit} />);
    const field = document.querySelector("input");

    await userEvent.clear(field);
    expect(screen.getByRole("button", { name: "Save" }).disabled).toBe(true);

    await userEvent.type(field, "  Work chat  ");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onSubmit).toHaveBeenCalledWith("chat-1", "Work chat");
  });

  it("saves on Enter", async () => {
    const onSubmit = vi.fn();
    render(<RenameDialog target={chat} onClose={() => {}} onSubmit={onSubmit} />);
    await userEvent.click(document.querySelector("input"));
    await userEvent.keyboard("{Enter}");
    expect(onSubmit).toHaveBeenCalledWith("chat-1", "Alice");
  });
});

describe("ConfirmDelete", () => {
  it("names the conversation and says the peer is not notified", () => {
    render(<ConfirmDelete target={chat} onClose={() => {}} onConfirm={() => {}} />);
    expect(screen.getByText(/can't be undone/)).toBeTruthy();
    expect(screen.getByText(/not notified/)).toBeTruthy();
  });

  it("only deletes on confirmation", async () => {
    const onConfirm = vi.fn();
    const onClose = vi.fn();
    render(<ConfirmDelete target={chat} onClose={onClose} onConfirm={onConfirm} />);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledWith("chat-1");
  });
});

describe("InfoDialog", () => {
  it("shows the grouped fingerprint and the transport in words", () => {
    render(<InfoDialog target={chat} onClose={() => {}} />);
    expect(screen.getByText(/direct \(peer-to-peer\)/)).toBeTruthy();
    // Grouped in fours so it can be read aloud without losing your place.
    expect(screen.getByText(/^abab abab/)).toBeTruthy();
  });

  it("says so plainly when there is no fingerprint yet", () => {
    render(<InfoDialog target={{ ...chat, fingerprint: "" }} onClose={() => {}} />);
    expect(screen.getByText("No fingerprint yet.")).toBeTruthy();
  });

  it("describes a relayed conversation as still end-to-end encrypted", () => {
    render(<InfoDialog target={{ ...chat, transport: "relay" }} onClose={() => {}} />);
    expect(screen.getByText(/blind broker, still end-to-end encrypted/)).toBeTruthy();
  });
});
