# Blink Trading Dashboard — Design System v2.0

## Overview
Professional, refined minimalist aesthetic for enterprise trading dashboard. Emphasis on clarity, precision, and zero-clutter financial UI.

## Color System

### Light Mode
```
Neutrals:
- bg-primary: #fafafa
- bg-secondary: #f5f5f5
- surface-primary: #ffffff
- border: #e0e0e0
- text-primary: #1a1a1a
- text-secondary: #666666
- text-muted: #999999

Semantic:
- success (buy): #059669
- danger (sell): #dc2626
- warning: #d97706
- info: #0891b2
- primary: #2563eb
```

### Dark Mode
```
- bg-primary: #0f0f0f
- bg-secondary: #1a1a1a
- surface-primary: #1a1a1a
- border: #333333
- text-primary: #f5f5f5
- text-secondary: #b3b3b3
```

## Typography

### Fonts
- Display: Space Grotesk (600 weight) — Headlines, precision
- Body: Inter Tight (400 weight) — Body text, clarity
- Mono: Jetbrains Mono (500 weight) — Numbers, code

### Scale (Modular 1.25x)
- 5xl: 3rem (48px) — Page titles
- 4xl: 2.25rem (36px) — Section headers
- 3xl: 1.875rem (30px) — Card titles
- 2xl: 1.5rem (24px) — Subsection headers
- xl: 1.25rem (20px) — Component headers
- lg: 1.125rem (18px) — Large text
- base: 1rem (16px) — Primary body
- sm: 0.875rem (14px) — Secondary text
- xs: 0.75rem (12px) — Labels
- 2xs: 0.625rem (10px) — Badge text

## Spacing (4px grid)
- 0: 0
- 1: 4px
- 2: 8px
- 3: 12px
- 4: 16px (primary)
- 5: 20px
- 6: 24px
- 8: 32px
- 10: 40px
- 12: 48px (section)

## Component Patterns

### Cards
- Rounded: 8px
- Padding: 24px (space-6)
- Border: 1px solid #e0e0e0
- Shadow: 0 1px 3px rgba(0,0,0,0.08)
- Hover shadow: 0 4px 12px rgba(0,0,0,0.1)

### Buttons
- Primary: Solid #2563eb, white text, 8px radius
- Secondary: Outlined #e0e0e0, dark text, 8px radius
- Tertiary: Text-only #2563eb, no background

### Data Display
- Use monospace for numbers
- Color + weight for hierarchy
- Labels: xs, muted, tracking-wide

## Accessibility
- Focus outline: 2px solid #2563eb, 2px offset
- Reduced motion support
- High contrast mode support
- Color + pattern (not just color)

## Motion
- Page transitions: 200ms cubic-bezier(0.2, 0, 0, 1)
- Hover states: 200ms smooth
- Data updates: Subtle pulse/flash

---

Generated: 2026-04-21
