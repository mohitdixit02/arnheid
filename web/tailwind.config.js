/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        crdb: {
          light: '#e0f2fe',  // Light blue
          sky: '#38bdf8',    // Sky blue
          blue: '#0284c7',   // Primary blue accent
          dark: '#0369a1',   // Dark blue
          gray: '#64748b',   // Slate gray
          darkgray: '#334155'
        }
      }
    },
  },
  plugins: [],
}
