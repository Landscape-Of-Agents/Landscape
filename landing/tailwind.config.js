/** Agent Landscape: black and grey glass, Geist for text, Instrument Serif for accents. */
/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./public/**/*.{html,js}"],
  theme: {
    extend: {
      fontFamily: {
        sans: ['"Geist"', "ui-sans-serif", "system-ui", "sans-serif"],
        serif: ['"Instrument Serif"', "ui-serif", "Georgia", "serif"],
        mono: ['"Geist Mono"', "ui-monospace", "SFMono-Regular", "Consolas", "monospace"],
      },
      colors: {
        ink: { DEFAULT: "#ededef", 2: "#a1a1a6", 3: "#6b6b70" },
        night: { DEFAULT: "#050506", 2: "#0b0b0d", 3: "#131316" },
      },
      maxWidth: { page: "68rem" },
      keyframes: {
        drift: { "0%, 100%": { transform: "translate3d(0,0,0) scale(1)" }, "50%": { transform: "translate3d(4%, -3%, 0) scale(1.08)" } },
        marquee: { from: { transform: "translateX(0)" }, to: { transform: "translateX(-50%)" } },
        rise: { from: { opacity: "0", transform: "translateY(14px)" }, to: { opacity: "1", transform: "none" } },
        caret: { "0%, 49%": { opacity: "1" }, "50%, 100%": { opacity: "0" } },
        shine: { from: { backgroundPosition: "200% 0" }, to: { backgroundPosition: "-200% 0" } },
      },
      animation: {
        drift: "drift 18s ease-in-out infinite",
        "drift-slow": "drift 26s ease-in-out infinite reverse",
        marquee: "marquee 32s linear infinite",
        rise: "rise .7s cubic-bezier(.2,.7,.2,1) both",
        caret: "caret 1s steps(1) infinite",
        shine: "shine 6s linear infinite",
      },
    },
  },
  plugins: [],
};
