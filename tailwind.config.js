/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // Verdict palette. false and misleading are BOTH red on purpose — amber
        // for "misleading" reads as "minor", which inverts the point (a misleading
        // claim is a true statement arranged to deceive: the harder kind to catch).
        verdict: {
          verified: "#22c55e",
          false: "#ef4444",
          misleading: "#ef4444",
          context: "#eab308",
          unverifiable: "#94a3b8",
        },
        panel: "rgba(18,18,20,0.82)",
      },
      fontFamily: {
        sans: ["-apple-system", "BlinkMacSystemFont", "SF Pro Text", "Inter", "system-ui", "sans-serif"],
      },
    },
  },
  plugins: [],
};
