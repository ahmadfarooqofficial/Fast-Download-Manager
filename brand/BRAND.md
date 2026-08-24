# FDM Brand Guidelines

**Product name:** FDM — Fast Download Manager
**Short name:** FDM
**Developer:** Ahmad Farooq
**Positioning:** A fast, free, open-source download manager for Windows.

> **Naming note.** "Fast Download Manager" is a distinct name, but the initialism `FDM`
> overlaps with Free Download Manager (SoftDeluxe). Consequence for every public surface:
> **always spell out "FDM — Fast Download Manager"** in store listings, the installer
> title, the window title, and the site `<title>`. Never ship a surface that says only
> "FDM", or users and store reviewers will read it as the other product.
> Background: [RESEARCH.md §1](../RESEARCH.md).

---

## 1. Positioning and voice

| | |
|---|---|
| **One-liner** | Downloads, in parallel. Free and open source. |
| **Elevator** | FDM splits every download across many connections, writes them straight to disk with no reassembly step, and resumes cleanly after a crash. Free, open source, no telemetry. |
| **Tone** | Precise, technical, unhyped. State throughput numbers, don't promise multiples. |
| **Never say** | "Up to 5× faster", "accelerate any download", "boost your internet speed". A download manager cannot exceed the line rate, and claiming otherwise is the single fastest way to lose the audience that actually chooses tools like this. |
| **Say instead** | "Keeps every connection busy until the last byte." "Resumes from disk, not from zero." |

---

## 2. Colour

Red and black, Netflix-like: near-black surfaces with one saturated red carrying every
active state. Red is an **accent, never a field** — a large red area reads as an error.

### Core

| Token | Hex | Use | Contrast on `#020617` |
|---|---|---|---|
| `--fdm-red` | `#E50914` | Progress fills, primary buttons, logo, active segment ticks | 4.21:1 — **fills and large text only** |
| `--fdm-red-text` | `#FF4B4B` | Red *text* at body size, links, small labels | 6.11:1 ✅ AA |
| `--fdm-red-hover` | `#FF1A25` | Hover on red surfaces | — |
| `--fdm-red-press` | `#B80710` | Pressed on red surfaces | — |
| `--fdm-on-red` | `#FFFFFF` | Text/icons **on** `--fdm-red` | 4.79:1 ✅ AA |

> The 4.21:1 figure is why there are two reds. `#E50914` is below the 4.5:1 threshold for
> normal-size text on near-black, so it is legal for fills, borders, icons, and large
> numerals (3:1 non-text / large-text threshold) but **not** for body copy. Body-size red
> text uses `#FF4B4B`.

### Surfaces and text

| Token | Hex | Use | Contrast |
|---|---|---|---|
| `--fdm-bg` | `#020617` | App background | — |
| `--fdm-surface` | `#0E1223` | Cards, list rows, panels | — |
| `--fdm-surface-2` | `#1A1E2F` | Hover rows, inputs, muted blocks | — |
| `--fdm-border` | `#334155` | Dividers, input borders | 3.1:1 vs `--fdm-surface` ✅ non-text |
| `--fdm-fg` | `#F8FAFC` | Primary text | 17.9:1 ✅ AAA |
| `--fdm-fg-muted` | `#94A3B8` | Secondary text, units, timestamps | 6.9:1 ✅ AA |
| `--fdm-fg-subtle` | `#64748B` | Disabled text only | 3.5:1 — never body copy |

### Status

Status must never be carried by colour alone — always pair with an icon or a word.

| Token | Hex | Meaning |
|---|---|---|
| `--fdm-success` | `#22C55E` | Completed |
| `--fdm-warning` | `#F59E0B` | Queued, paused, retrying |
| `--fdm-danger` | `#EF4444` | Failed |
| `--fdm-info` | `#38BDF8` | Informational |

`--fdm-danger` and `--fdm-red` are deliberately different values. Brand red is
`#E50914`; failure red is `#EF4444`. If a failed row and a progress bar were the same red,
a running download would look broken.

### Provenance

Structure (surface ladder, border, foreground, muted foreground, spacing, Inter) comes from
the `ui-ux-pro-max` skill's verified dashboard palette. **Documented override:** that
dataset's accent is green (`#16A34A` / `#22C55E`); it has no red-accent dark palette. The
red accent is a brand decision by the developer and supersedes the dataset recommendation.
Green is retained, unchanged, as the *success* token only.

---

## 3. Typography

**Inter** throughout — heading and body. Verified match from the skill's typography set for
dashboards and professional tools.

| Role | Size | Weight | Notes |
|---|---|---|---|
| Display | 32px | 700 | Empty states, onboarding only |
| Title | 24px | 600 | Window/section headings |
| Subtitle | 18px | 600 | Panel headings |
| Body | 14px | 400 | Desktop-dense default |
| Body large | 16px | 400 | Settings descriptions, prose |
| Label | 13px | 500 | Buttons, table headers |
| Caption | 12px | 400 | Units, timestamps |

**Tabular figures are mandatory** for speeds, sizes, ETAs, and percentages:

```css
font-variant-numeric: tabular-nums;
```

Without this, `9.87 MB/s` and `10.02 MB/s` have different widths and the whole download
list twitches on every progress tick. This is the single most visible polish detail in a
download manager.

Line height 1.5 for prose, 1.3 for dense table rows.

---

## 4. Logo

Three elements, one idea: **a download arrow built out of parallel streams.** The centre
stem and arrowhead read as "download"; the two flanking bars read as "in parallel".

### Files

| File | Use |
|---|---|
| `logo/fdm-mark.svg` | The red mark alone, transparent. Use on any dark surface. |
| `logo/fdm-icon.svg` | The mark on a black squircle. Source for the app icon, `.ico`, tray, extension icon. |
| `logo/fdm-logo-horizontal.svg` | Icon + "FDM" + "FAST DOWNLOAD MANAGER" subline. README, site header, installer banner. |
| `logo/fdm-logo-stacked.svg` | Vertical lockup for square spaces and the installer splash. |
| `logo/fdm-mark-small.svg` | Arrow-only variant for anything under 24px — tray, extension toolbar, favicon. |

### Construction

Drawn on a 512×512 grid. Mark occupies y 116–399, optically centred.

```
        ▌ ▌ ▌      three bars, 52 wide, 8 gaps, centre bar longest
          ▌
        ╲ ▌ ╱      arrowhead, 52 stroke, round joins
          ▼
```

- Bars: 52px wide, fully rounded caps (`rx=26`).
- Centre stem runs into the arrowhead vertex, so they read as one solid arrow.
- Flanking bars stop 20px clear of the arrowhead arms — that gap is load-bearing, it's what
  keeps the mark from turning into a solid triangle at small sizes.
- Squircle corner radius 112 on 512 (≈22%).

### Clear space and minimum size

- Clear space on all sides = the width of one bar (52 units, ≈10% of the icon).
- Minimum icon size: **16px**. Below 24px use `logo/fdm-mark-small.svg`, which drops the
  flanking bars entirely and thickens the arrow (80 wide, 80 stroke) to hold the same visual
  weight — at 16px the 8px gaps collapse and the three bars smear into a solid block.
- Never place the mark on red. Black or `--fdm-bg` only. On a light background, use the
  black squircle version rather than recolouring the mark.

### Misuse

Do not: recolour the mark to anything but `#E50914` · add gradients, bevels, or glow ·
outline it · rotate it · stretch it · put a drop shadow on it · use the mark as a loading
spinner · set the wordmark in anything but Inter.

---

## 5. Iconography

- **Phosphor** (`@phosphor-icons/react`) at 1.5px stroke, per the skill's default.
- One weight per hierarchy level. No mixing filled and outline in the same row.
- Sizes are tokens only: `icon-sm 16`, `icon-md 20`, `icon-lg 24`. A dense desktop list
  uses 16; toolbar uses 20.
- **No emoji anywhere in the product UI.**
- Icon-only buttons need an `aria-label` *and* a tooltip.

---

## 6. Application

### Progress bars

The one place red is unmissable. Track `--fdm-surface-2`, fill `--fdm-red`, 4px tall in
list rows, 6px in the detail panel, 2px radius. Segment boundaries drawn as 1px
`--fdm-bg` ticks over the fill, so the user can literally see the parallel connections
working — that is the product's whole thesis rendered as one UI element.

### States

| State | Treatment |
|---|---|
| Downloading | Red fill, animated segment ticks, tabular speed |
| Paused | Fill drops to `--fdm-red` at 40% opacity, pause icon, "Paused" label |
| Completed | Fill becomes `--fdm-success`, check icon |
| Failed | Fill becomes `--fdm-danger`, alert icon, "Failed" + reason |
| Queued | Empty track, `--fdm-warning` dot, position number |

### Motion

Subtle tier (dial 3/10). 200–250ms, `power1.out`. Progress bars update by width transition
only — never re-render the row. Respect `prefers-reduced-motion`: skip the fill animation
and jump straight to the value.

---

## 7. Do not ship without

- [ ] Full name spelled out on every public surface
- [ ] Tabular figures on every number that updates
- [ ] Red never used as a large background field
- [ ] Body-size red text uses `--fdm-red-text`, not `--fdm-red`
- [ ] Status conveyed by icon + word, not colour alone
- [ ] Visible focus ring on every interactive element (2px, `--fdm-red`, 2px offset)
- [ ] Phosphor SVG icons only, no emoji
- [ ] Mark legible at 16px in the tray and the extension toolbar
