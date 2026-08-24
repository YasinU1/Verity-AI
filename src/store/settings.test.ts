import { describe, it, expect } from "vitest";
import { clampVerifyWords, DEFAULT_SETTINGS, VERIFY_WORDS_MAX, VERIFY_WORDS_MIN } from "./settings";

describe("settings", () => {
  it("ships auto-start OFF (a browser is frontmost most of the day)", () => {
    expect(DEFAULT_SETTINGS.autoStartEnabled).toBe(false);
  });

  it("defaults audio source to system", () => {
    expect(DEFAULT_SETTINGS.audioSource).toBe("system");
  });

  it("defaults verifyEveryWords to 50", () => {
    expect(DEFAULT_SETTINGS.verifyEveryWords).toBe(50);
  });

  it("clamps verifyEveryWords into the configurable range", () => {
    expect(clampVerifyWords(5)).toBe(VERIFY_WORDS_MIN);
    expect(clampVerifyWords(500)).toBe(VERIFY_WORDS_MAX);
    expect(clampVerifyWords(73)).toBe(73);
    expect(clampVerifyWords(73.6)).toBe(74);
    expect(clampVerifyWords(NaN)).toBe(50);
  });

  it("defaults the provider to auto", () => {
    expect(DEFAULT_SETTINGS.verificationProvider).toBe("auto");
  });
});
