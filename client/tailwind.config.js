/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        discord: {
          dark: '#09130e',       // Deepest obsidian dark-green
          sidebar: '#101e16',    // Dark forest pine
          chat: '#14251c',       // Rich emerald night
          hover: '#1b3325',      // Soft pine hover
          active: '#234130',     // Active channel selection
          input: '#192c21',      // Message input background
          border: '#233c2e',     // Elegant forest borders
          blurple: '#10b981',    // Vibrant Emerald 500
          'blurple-hover': '#059669', // Emerald 600
          green: '#34d399',      // Bright mint for speaking indicator and online status
          red: '#ef4444',        // Crisp modern red
          yellow: '#f59e0b',     // Warm amber
          text: {
            normal: '#d1fae5',   // High-readability pale mint
            muted: '#7fa490',    // Calming sage gray
            heading: '#f0fdf4',  // Pure crisp mint-white
          },
        },
      },
      boxShadow: {
        'emerald-glow': '0 0 20px -3px rgba(16, 185, 129, 0.4)',
        'speaking-glow': '0 0 15px 2px rgba(52, 211, 153, 0.6)',
      },
    },
  },
  plugins: [],
}
