/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{html,js,svelte,ts}', './index.html'],
  theme: {
    extend: {
      colors: {
        discord: {
          dark: '#1e1f22',
          sidebar: '#2b2d31',
          chat: '#313338',
          hover: '#35373c',
          active: '#404249',
          blurple: '#5865f2',
          'blurple-hover': '#4752c4',
          green: '#23a55a',
          red: '#f23f43',
          yellow: '#f0b232',
          text: '#dbdee1',
          'text-muted': '#949ba4',
          'text-header': '#f2f3f5',
        },
      },
    },
  },
  plugins: [],
};
