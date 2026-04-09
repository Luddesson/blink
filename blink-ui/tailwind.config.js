/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        // Paper mode accent — indigo
        paper: {
          400: '#818cf8',
          500: '#6366f1',
          600: '#4f46e5',
        },
        // Live mode accent — amber/red
        live: {
          400: '#fbbf24',
          500: '#f59e0b',
          danger: '#ef4444',
        },
        // Background layers
        surface: {
          950: '#060911',
          900: '#0b0f19',
          800: '#111827',
          700: '#1f2937',
          600: '#374151',
        },
        // Semantic trading colors
        bull:   '#22c55e',
        bear:   '#ef4444',
        signal: '#3b82f6',
        whale:  '#f59e0b',
      },
      fontFamily: {
        mono: ['JetBrains Mono', 'Fira Code', 'Consolas', 'monospace'],
      },
      animation: {
        'pulse-red': 'pulse 1s cubic-bezier(0.4, 0, 0.6, 1) infinite',
        'fade-in': 'fadeIn 0.2s ease-out',
      },
      keyframes: {
        fadeIn: {
          '0%': { opacity: '0', transform: 'translateY(4px)' },
          '100%': { opacity: '1', transform: 'translateY(0)' },
        },
      },
    },
  },
  plugins: [],
}
