# NativeLink Design System

**Design Direction:** Technical Warmth  
**Version:** 1.0  
**Last Updated:** 2026-05-01

## Overview

The NativeLink design system balances technical precision with approachability. Inspired by pi.website's minimal aesthetic but softer and more welcoming, it uses monospace typography, warm neutral backgrounds, and subtle visual guides to create a professional yet accessible experience.

## Design Principles

1. **Technical but Approachable** — Monospace fonts signal technical expertise, but soft colors and rounded elements make it welcoming
2. **Clear Segmentation** — Subtle accent lines separate sections without harsh borders
3. **Minimal Decoration** — Every visual element serves a purpose; no decoration for decoration's sake
4. **High Contrast Text** — Black text on warm backgrounds ensures readability
5. **Touch-Friendly** — All interactive elements meet 44px minimum touch targets (WCAG AAA)

## Color Palette

### Primary Colors

```css
--color-background: rgb(248, 247, 244);  /* Warm off-white, softer than pure white */
--color-foreground: rgb(0, 0, 0);       /* Pure black for maximum contrast */
--color-muted: rgb(100, 100, 100);      /* Secondary text */
--color-border: rgb(220, 220, 220);     /* Card borders and dividers */
--color-accent: rgb(0, 0, 0);           /* CTA buttons */
--color-accent-hover: rgb(40, 40, 40);  /* Button hover state */
--color-accent-line: rgb(180, 180, 180); /* Subtle section dividers */
```

### Usage Guidelines

- **Background:** Always use `--color-background` (rgb(248, 247, 244)) for page backgrounds
- **Text:** Primary text is pure black (`--color-foreground`), secondary text uses `--color-muted` or rgb(60, 60, 60)
- **Borders:** Use 2px borders in `--color-border` for cards and interactive elements
- **Section Dividers:** 1px lines in `--color-accent-line` (rgb(180, 180, 180))

## Typography

### Font Family

```css
font-family: ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New', monospace;
```

Monospace creates a technical, developer-focused aesthetic while remaining highly readable.

### Type Scale

Using a **1.333 ratio (perfect fourth)** for consistent hierarchy:

```css
h1: 96px   (font-weight: 700, line-height: 1.15)
h2: 72px   (font-weight: 700, line-height: 1.2)
h3: 54px   (font-weight: 600, line-height: 1.25)
h4: 40.5px (font-weight: 600, line-height: 1.3)
h5: 30px   (font-weight: 600, line-height: 1.4)
h6: 24px   (font-weight: 600, line-height: 1.4)
Body: 16px (font-weight: 400, line-height: 1.7)
Small: 14px (font-weight: 400, line-height: 1.6)
```

### Mobile Adjustments

- h1: 48px (50% of desktop size)
- h2: 36px
- Body and other sizes remain the same

## Spacing

### Vertical Rhythm

Consistent spacing creates visual hierarchy and breathing room:

- **Section padding:** 80px top/bottom (120px for hero)
- **Element spacing:** Use multiples of 8px (8, 16, 24, 32, 48, 64)
- **Line height:** Body text uses 1.7 for readability, headlines use tighter 1.15-1.3

### Layout Constraints

- **Max content width:** 1200px for features/content grids
- **Hero content:** Max 850px for headlines, 550px for subtitle
- **Horizontal padding:** 24px on mobile, responsive on desktop

## Components

### Buttons (CTA)

```css
/* Primary Button */
background-color: rgb(0, 0, 0);
color: white;
border: 2px solid rgb(0, 0, 0);
border-radius: 4px;
padding: 14px-16px 40px-48px;
min-height: 48px; /* Touch target */
transition: background-color 0.2s;

/* Hover State */
background-color: rgb(40, 40, 40);
transform: translateY(-1px); /* Optional subtle lift */
```

**Key Details:**
- Always rounded corners (4px)
- 2px borders for definition
- Minimum 48px touch target height
- Smooth hover transition to rgb(40, 40, 40)

### Cards

```css
/* Feature/Content Cards */
padding: 24px;
border: 2px solid rgb(220, 220, 220);
border-radius: 4px;
background-color: rgba(255, 255, 255, 0.5); /* Semi-transparent white */
```

**Key Details:**
- 2px borders (not 1px) for clarity
- 4px rounded corners
- Semi-transparent white background adds subtle depth
- Use `.card-warm` utility class

### Section Dividers

```css
.section-divider::after {
  content: '';
  position: absolute;
  bottom: 0;
  left: 10%;
  right: 10%;
  height: 1px;
  background-color: rgb(180, 180, 180);
}
```

**Key Details:**
- 1px lines (subtle, not harsh)
- Inset 10% from each side (not full width)
- Applied to sections needing visual separation

### Video/Media Elements

```css
border: 2px solid rgb(220, 220, 220);
border-radius: 4px;
box-shadow: 0px 0px 50px 0px rgba(96, 80, 230, 0.3); /* Existing purple glow */
```

**Key Details:**
- Maintain existing shadow for brand continuity
- Add 2px border and 4px rounded corners for cohesion

## Accessibility

### Touch Targets (WCAG AAA)

- **Minimum size:** 48px × 48px for all interactive elements
- **Spacing:** Minimum 8px between adjacent touch targets
- **Visual feedback:** Hover states on all clickable elements

### Contrast Ratios

- **Body text (16px):** Black on rgb(248, 247, 244) = 18.9:1 (exceeds WCAG AAA 7:1)
- **Large text (24px+):** 18.9:1 (exceeds WCAG AAA 4.5:1)
- **Buttons:** White on black = 21:1 (maximum contrast)

### Text Wrapping

Use `text-wrap: balance` on headlines to prevent awkward line breaks.

## Implementation

### Tailwind CSS Classes

The design system is implemented via Tailwind v4 CSS-first in `styles/tailwind.css`:

```css
/* Custom utility classes */
.rounded-interactive { border-radius: 4px; }
.card-warm { 
  border: 2px solid var(--color-border);
  border-radius: 4px;
  background-color: rgba(255, 255, 255, 0.5);
}
.section-divider { /* applies divider line via ::after */ }
```

### Component Usage

**Hero Section:**
- Rounded CTA button with 4px corners
- Video demo with 2px border and 4px corners
- Centered layout with max-width constraints

**Feature Cards:**
- Use `.card-warm` class for consistent styling
- 24px padding, 2px borders, 4px corners
- Semi-transparent white background

**Section Separators:**
- Add `.section-divider` class to section containers
- Automatically adds subtle 1px divider line at bottom

## Design Rationale

### Why "Technical Warmth"?

NativeLink serves developers and infrastructure teams who value:
1. **Technical credibility** — Monospace fonts signal we understand their world
2. **Professional polish** — Clean layout and consistent spacing show attention to detail
3. **Approachability** — Softer colors and rounded elements make the platform feel accessible, not intimidating

### Avoiding Pitfalls

❌ **Rejected: Swiss Precision (Variant B)**  
Too stark and spartan. Pure black/white felt cheap despite mathematical precision.

❌ **Rejected: Brutalist Minimal (Variant D)**  
Heavy 3px black borders felt aggressive. Not the welcoming tone we want.

✅ **Chosen: Technical Warmth (Variant C)**  
Strikes the balance between technical and approachable. Clear segmentation without harsh borders.

## Visual Examples

### Hero Section
- 96px headline in monospace
- Warm beige background (248, 247, 244)
- Black CTA button with 4px rounded corners
- Video demo with subtle border and shadow

### Feature Grid
- 4-column responsive grid (1 column on mobile)
- Cards with 2px borders and semi-transparent backgrounds
- 48px gap between cards
- Consistent 24px internal padding

### Company Logos Section
- Subtle 1px divider line above and below
- Centered logos with flexible wrapping
- Opacity 0.6-0.7 for visual hierarchy

## Migration Notes

Changed from previous pi.website variant:
1. Background lightened from rgb(245, 244, 239) → rgb(248, 247, 244)
2. All buttons now have 4px rounded corners (was 0px)
3. Feature cards now have 2px borders and semi-transparent backgrounds
4. Video/media elements have 2px borders and 4px corners
5. Section dividers added via `.section-divider` class (1px subtle lines)
6. Hover color updated to rgb(40, 40, 40) for smoother transition

## Future Considerations

- **Dark Mode:** If needed, invert to dark background with light text while maintaining warmth
- **Animation:** Consider subtle transitions on card hover (slight lift, shadow growth)
- **Iconography:** If adding icons, use outlined style at 24px minimum for touch targets
- **Component Library:** Document interactive states (focus, active, disabled) as components grow

---

**Questions or Feedback?**  
Design system maintained by the NativeLink team. For questions or proposed changes, open an issue on GitHub.
