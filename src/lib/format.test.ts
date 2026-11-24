import { describe, expect, it } from "vitest";
import { colorFor, displayName, fileSize, formatAddr, initials } from "./format";

describe("format helpers", () => {
  it("formats addresses and display names", () => {
    expect(formatAddr({ name: "Alice", email: "alice@example.com" })).toBe("Alice <alice@example.com>");
    expect(formatAddr({ name: null, email: "alice@example.com" })).toBe("alice@example.com");
    expect(displayName("  ", "alice@example.com")).toBe("alice@example.com");
    expect(displayName("Alice", "alice@example.com")).toBe("Alice");
  });

  it("formats file sizes", () => {
    expect(fileSize(512)).toBe("512 B");
    expect(fileSize(2048)).toBe("2 KB");
    expect(fileSize(2.5 * 1024 * 1024)).toBe("2.5 MB");
  });

  it("derives initials and deterministic colors", () => {
    expect(initials("Ada Lovelace", "ada@example.com")).toBe("AL");
    expect(initials("", "bob@example.com")).toBe("BO");
    expect(colorFor("a@b.com")).toBe(colorFor("a@b.com"));
  });
});
