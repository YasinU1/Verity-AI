// Settings store (spec §12). Zustand + persist with versioned migrations.
//
// Honest note on migrations: persisted state CANNOT distinguish "the user explicitly
// chose X" from "the user inherited the old default X". So a migration that changes a
// default changes it for everyone who never touched that setting — there is no way to
// tell those two populations apart from the persisted blob alone. We do not pretend
// otherwise anywhere in this file.

import { create } from "zustand";
import { persist } from "zustand/middleware";

export type AudioSource = "system" | "microphone";
export type VerificationProvider = "auto" | "openai" | "anthropic";
export type Sensitivity = "low" | "medium" | "high";

export interface Settings {
  audioSource: AudioSource;
  audioDeviceId: string | null;
  visionEnabled: boolean;
  visionIntervalMs: number;
  overlayEnabled: boolean;
  overlayClickThrough: boolean;
  verifyEveryWords: number; // 20..200, default 50
  sensitivity: Sensitivity;
  webSearchEnabled: boolean;
  autoStartEnabled: boolean; // ships OFF (spec §10) — a browser is frontmost all day
  openAiApiKey: string;
  anthropicApiKey: string;
  exaApiKey: string;
  verificationProvider: VerificationProvider;
  transcriptionModel: string;
  verificationModel: string; // OpenAI verification model
  anthropicModel: string;
}

// Dev-only fallback to VITE_* env vars so a developer needn't paste keys into the UI.
// These never reach a bundled build unless the env var was set at build time.
const env = (k: string): string =>
  (import.meta as unknown as { env?: Record<string, string> }).env?.[k] ?? "";

export const DEFAULT_SETTINGS: Settings = {
  audioSource: "system",
  audioDeviceId: null,
  visionEnabled: false,
  visionIntervalMs: 4000,
  overlayEnabled: true,
  overlayClickThrough: true,
  verifyEveryWords: 50,
  sensitivity: "medium",
  webSearchEnabled: true,
  autoStartEnabled: false,
  openAiApiKey: env("VITE_OPENAI_API_KEY"),
  anthropicApiKey: env("VITE_ANTHROPIC_API_KEY"),
  exaApiKey: env("VITE_EXA_API_KEY"),
  verificationProvider: "auto",
  transcriptionModel: "gpt-live-transcribe",
  verificationModel: "gpt-4o-mini",
  anthropicModel: "claude-haiku-4-5",
};

export const VERIFY_WORDS_MIN = 20;
export const VERIFY_WORDS_MAX = 200;

export function clampVerifyWords(n: number): number {
  if (!Number.isFinite(n)) return DEFAULT_SETTINGS.verifyEveryWords;
  return Math.min(VERIFY_WORDS_MAX, Math.max(VERIFY_WORDS_MIN, Math.round(n)));
}

interface SettingsStore extends Settings {
  set: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  reset: () => void;
  /** Keys usable by the backend right now (UI value OR dev env fallback). */
  effectiveKeys: () => { openai: string; anthropic: string; exa: string };
}

export const SETTINGS_VERSION = 2;

export const useSettings = create<SettingsStore>()(
  persist(
    (setState, getState) => ({
      ...DEFAULT_SETTINGS,
      set: (key, value) => {
        const v = key === "verifyEveryWords" ? (clampVerifyWords(value as number) as Settings[typeof key]) : value;
        setState({ [key]: v } as Partial<Settings>);
      },
      reset: () => setState({ ...DEFAULT_SETTINGS }),
      effectiveKeys: () => {
        const s = getState();
        return {
          openai: s.openAiApiKey || env("VITE_OPENAI_API_KEY"),
          anthropic: s.anthropicApiKey || env("VITE_ANTHROPIC_API_KEY"),
          exa: s.exaApiKey || env("VITE_EXA_API_KEY"),
        };
      },
    }),
    {
      name: "verity-settings",
      version: SETTINGS_VERSION,
      // Migrations run oldest→newest. Adding a field is safe (spread defaults);
      // CHANGING a default is not silent — see the honest note at the top of the file.
      migrate: (persisted, fromVersion) => {
        const state = { ...DEFAULT_SETTINGS, ...(persisted as Partial<Settings>) };
        if (fromVersion < 1) {
          // v0→v1: introduced verifyEveryWords; clamp whatever was there.
          state.verifyEveryWords = clampVerifyWords(state.verifyEveryWords);
        }
        if (fromVersion < 2) {
          // v1→v2: split verification model into per-provider fields. Anyone who never
          // set a provider model inherits the new defaults — indistinguishable from an
          // explicit choice, by construction.
          if (!state.anthropicModel) state.anthropicModel = DEFAULT_SETTINGS.anthropicModel;
        }
        return state as SettingsStore;
      },
      // Never persist secrets you don't have to? We DO persist keys (the user pasted
      // them and expects them to stick), but only in the app's own local store.
    },
  ),
);
