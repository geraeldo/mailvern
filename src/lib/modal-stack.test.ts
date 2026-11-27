import { describe, expect, it, vi } from "vitest";
import { closeTopModal, registerModal } from "./modal-stack";

describe("modal-stack", () => {
  it("returns false when no modal is registered", () => {
    expect(closeTopModal()).toBe(false);
  });

  it("closes only the topmost modal in LIFO order", () => {
    const first = vi.fn();
    const second = vi.fn();
    const unFirst = registerModal(first);
    const unSecond = registerModal(second);

    expect(closeTopModal()).toBe(true);
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();

    unSecond();
    expect(closeTopModal()).toBe(true);
    expect(first).toHaveBeenCalledTimes(1);

    unFirst();
    expect(closeTopModal()).toBe(false);
  });

  it("handles out-of-order unregistration safely", () => {
    const first = vi.fn();
    const second = vi.fn();
    const unFirst = registerModal(first);
    const unSecond = registerModal(second);

    unFirst();
    expect(closeTopModal()).toBe(true);
    expect(second).toHaveBeenCalledTimes(1);
    expect(first).not.toHaveBeenCalled();

    unSecond();
    expect(closeTopModal()).toBe(false);
  });
});
