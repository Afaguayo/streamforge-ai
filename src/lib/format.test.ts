import { describe, expect, it } from "vitest";
import { ago, bytes, duration } from "./format";

describe("format", () => {
  it("formats durations", () => {
    expect(duration(65)).toBe("1:05");
    expect(duration(7265.4)).toBe("2:01:05");
    expect(duration(null)).toBe("—");
  });

  it("formats sizes", () => {
    expect(bytes(512)).toBe("512 B");
    expect(bytes(4.2e9)).toBe("3.91 GB");
  });

  it("formats relative times", () => {
    const now = Date.parse("2026-10-29T12:00:00Z");
    expect(ago("2026-10-29T11:59:50Z", now)).toBe("just now");
    expect(ago("2026-10-29T11:30:00Z", now)).toBe("30 min ago");
    expect(ago("2026-10-27T12:00:00Z", now)).toBe("2 d ago");
    expect(ago(null, now)).toBe("never");
  });
});
