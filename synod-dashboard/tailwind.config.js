/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        synod: {
          accent: "#00ffcc",
          error: "#ff3366",
          bg: "#050505",
          card: "rgba(255, 255, 255, 0.05)",
        }
      }
    },
  },
  plugins: [],
}
