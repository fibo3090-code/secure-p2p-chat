// @vitest-environment jsdom
//
// The screens a user meets before anything else. `BootScreen` matters most: it
// used to render `null`, so one slow or failed `auth_status` left a permanently
// blank window. Its `fatal` branch is a security decision made visible — the app
// refuses to replace an identity it merely failed to read, and has to say so
// rather than silently starting over as a stranger to every contact.

import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BootScreen, LockScreen, SetPasswordScreen, BackupPrompt } from "./Onboarding.jsx";

describe("BootScreen", () => {
  it("shows progress rather than an empty window while the bridge is slow", () => {
    const { container } = render(<BootScreen retrying />);
    expect(container.querySelector(".onb-card")).toBeTruthy();
  });

  it("surfaces a failure and offers a retry", async () => {
    const onRetry = vi.fn();
    render(<BootScreen error="The backend did not respond." onRetry={onRetry} />);
    expect(screen.getByText(/did not respond/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /try again/i }));
    expect(onRetry).toHaveBeenCalled();
  });

  it("offers no retry for an unreadable identity, and says nothing was deleted", () => {
    render(<BootScreen fatal error="identity.json exists but could not be read" />);
    // Retrying cannot fix this, and creating a replacement identity would
    // abandon the history and every verified fingerprint.
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
    expect(screen.getByText(/Nothing has been changed or deleted/)).toBeTruthy();
    expect(screen.getByText(/could not be read/)).toBeTruthy();
  });
});

describe("LockScreen", () => {
  it("passes the password to the bridge and shows what it says back", async () => {
    const onUnlock = vi.fn(async () => "Incorrect password");
    render(<LockScreen onUnlock={onUnlock} />);
    await userEvent.type(document.querySelector("input"), "correct-horse-battery");
    await userEvent.click(screen.getByRole("button", { name: /unlock/i }));

    expect(onUnlock).toHaveBeenCalledWith("correct-horse-battery");
    expect(await screen.findByText("Incorrect password")).toBeTruthy();
  });

  it("does not submit an empty password", async () => {
    const onUnlock = vi.fn(async () => "");
    render(<LockScreen onUnlock={onUnlock} />);
    await userEvent.click(screen.getByRole("button", { name: /unlock/i }));
    expect(onUnlock).not.toHaveBeenCalled();
  });
});

describe("SetPasswordScreen", () => {
  it("refuses a password under the floor the backend publishes", async () => {
    const onSet = vi.fn(async () => "");
    render(<SetPasswordScreen minLength={12} onSet={onSet} />);
    const [pw, pw2] = document.querySelectorAll("input");
    await userEvent.type(pw, "short");
    await userEvent.type(pw2, "short");
    await userEvent.click(screen.getByRole("button", { name: /Create identity/ }));
    // MIN_PASSWORD_LEN is enforced in Rust regardless; the UI validates against
    // the real rule so the user is told before the round-trip.
    expect(onSet).not.toHaveBeenCalled();
  });

  it("flags a mismatch before submitting", async () => {
    const onSet = vi.fn(async () => "");
    render(<SetPasswordScreen minLength={12} onSet={onSet} />);
    const [pw, pw2] = document.querySelectorAll("input");
    await userEvent.type(pw, "correct-horse-battery");
    await userEvent.type(pw2, "correct-horse-batteries");
    expect(screen.getByText(/Passwords don't match/)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Create identity/ }));
    expect(onSet).not.toHaveBeenCalled();
  });

  it("submits a valid password and reports a backend refusal", async () => {
    const onSet = vi.fn(async () => "keystore is read-only");
    render(<SetPasswordScreen minLength={12} onSet={onSet} />);
    const [pw, pw2] = document.querySelectorAll("input");
    await userEvent.type(pw, "correct-horse-battery");
    await userEvent.type(pw2, "correct-horse-battery");
    await userEvent.click(screen.getByRole("button", { name: /Create identity/ }));

    expect(onSet).toHaveBeenCalledWith("correct-horse-battery");
    expect(await screen.findByText("keystore is read-only")).toBeTruthy();
  });

  it("shows the new identity's fingerprint", () => {
    render(<SetPasswordScreen minLength={12} fingerprint={"ab".repeat(32)} onSet={async () => ""} />);
    expect(screen.getByText(/^abababab/)).toBeTruthy();
  });
});

describe("BackupPrompt", () => {
  it("confirms where the backup landed", async () => {
    const onExport = vi.fn(async () => "/home/me/p2pem-identity-backup.json");
    render(<BackupPrompt onExport={onExport} onSkip={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /Save backup file/ }));
    expect(await screen.findByText(/Saved to \/home\/me/)).toBeTruthy();
  });

  it("reports a failed export instead of pretending it worked", async () => {
    render(<BackupPrompt onExport={async () => { throw new Error("permission denied"); }} onSkip={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /Save backup file/ }));
    expect(await screen.findByText(/permission denied/)).toBeTruthy();
  });

  it("can be skipped — the prompt must not be a wall", async () => {
    const onSkip = vi.fn();
    render(<BackupPrompt onExport={async () => ""} onSkip={onSkip} />);
    await userEvent.click(screen.getByRole("button", { name: /Skip for now/ }));
    expect(onSkip).toHaveBeenCalled();
  });
});
