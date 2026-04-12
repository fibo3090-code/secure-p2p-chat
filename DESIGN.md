# Design System Specification: The Command & Control Interface

## 1. Overview & Creative North Star: "The Digital Hive-Mind"
This design system is not a collection of UI components; it is a sophisticated "Mission Control" environment. Our Creative North Star is **The Sovereign Architect**. It represents a shift from consumer-grade "apps" to professional-grade "instruments."

To achieve this, we move beyond the generic "SaaS" look. We utilize **Intentional Asymmetry** and **Tonal Layering** to create a sense of focused power. The interface should feel like a high-performance HUD—secure, local-first, and hyper-intelligent. We break the grid with overlapping "data-panes" and use high-contrast typography to ensure that even in high-density environments, the most critical "intel" is immediately actionable.

---

## 2. Colors & Surface Philosophy
Our palette is rooted in the "void"—a deep, charcoal space where information is illuminated by the warmth of `primary` and the precision of `secondary`.

### The "No-Line" Rule
**Explicit Instruction:** Designers are prohibited from using 1px solid borders to define sections or containers. Structural integrity must be achieved through:
1. **Background Shifts:** Placing a `surface_container_low` card on top of a `surface` background.
2. **Negative Space:** Using our spacing scale (e.g., `spacing.8`) to allow sections to breathe.
3. **Tonal Transitions:** Subtle shifts in container hierarchy to denote importance.

### Surface Hierarchy & Nesting
Treat the UI as a physical stack of semi-translucent materials.
* **Base:** `background` (#0e0e0e) – The canvas.
* **Foundation:** `surface_container_low` (#131313) – Main content areas.
* **Actionable:** `surface_container` (#1a1a1a) – Interactive cards or utility panels.
* **High-Priority:** `surface_container_high` (#20201f) – Pop-overs or focused agent nodes.

### The "Glass & Gradient" Rule
To add "soul" to the high-tech aesthetic:
* **Floating Elements:** Use semi-transparent variants of `surface_variant` with a 20px-40px `backdrop-blur` to create a "Cyber-Glass" effect for overlays.
* **Signature Gradients:** Use a subtle linear gradient from `primary` (#ffd16c) to `primary_container` (#fdc003) on high-level CTAs to simulate the glow of an active AI core.

---

## 3. Typography
We utilize a dual-font strategy to balance editorial sophistication with technical precision.

* **Display & Headlines (`Space Grotesk`):** This is our "command" typeface. Its wide stance and geometric quirks feel intentional and high-tech. Use `display-lg` for dashboard titles and `headline-sm` for section headers.
* **Interface & Data (`Inter`):** Our workhorse. Used for `body` and `label` styles. It ensures high readability at small sizes, crucial for high-density AI logs.
* **Hierarchy Note:** Use `on_surface_variant` (muted gray) for metadata and `on_surface` (pure white) for active content to create immediate visual scannability.

---

## 4. Elevation & Depth
In this system, depth is a function of light and material, not artificial shadows.

* **Tonal Layering:** Avoid drop shadows for standard UI elements. A `surface_container_highest` element sitting on a `surface_dim` background is enough to indicate elevation.
* **Ambient Shadows:** For floating modals, use a "Ghost Shadow."
* *Blur:* 40px | *Opacity:* 6% | *Color:* Tincture of `surface_tint`.
* **The "Ghost Border" Fallback:** If accessibility requires a border, use `outline_variant` at **15% opacity**. It should be felt, not seen.
* **Hex-Grid Patterning:** Apply a subtle SVG hex-grid mask over `surface_container_lowest` backgrounds (opacity 3-5%) to reinforce the "Hive" concept without distracting from data.

---

## 5. Components

### Buttons
* **Primary:** A solid block of `primary` with `on_primary_fixed` text. No border. Radius: `rounded.md`.
* **Secondary (Cyber-Glass):** A `surface_variant` background with a subtle `primary` inner glow (0.5px inset).
* **Tertiary:** Purely typographic using `label-md` in `primary` color, no background container.

### Input Fields
* **Styling:** Use `surface_container_highest` as the field background.
* **Active State:** No heavy border; instead, use a 1px `primary` underline or a subtle `primary` outer glow to indicate focus.
* **Data Density:** Maintain tight padding (e.g., `spacing.2.5`) for a compact, professional feel.

### Cards & Lists
* **Rule:** **No Divider Lines.** Use `spacing.4` vertical gaps or alternating shifts between `surface_container_low` and `surface_container` to distinguish between list items.
* **Rich Media:** AI Agent avatars or status nodes should use `secondary` (Neon Green) for active pulses.

### AI Agent Nodes (Custom Component)
* A specialized container using `surface_container_high`, featuring a `primary` corner accent and a `Roboto Mono` string representing the agent's current process ID.

---

## 6. Do’s and Don’ts

### Do
* **Do** embrace dark space. The "security" of the platform is felt through the depth of the charcoal palette.
* **Do** use `secondary` (Neon Green) sparingly. It is a "Signal" color, reserved for success or active agent states.
* **Do** align all elements to the `spacing` scale to maintain "Mission Control" precision.

### Don't
* **Don't** use `rounded.full` (pills) for buttons. Use `rounded.sm` or `rounded.md` to maintain a professional, architectural edge.
* **Don't** use pure black (#000000) for surfaces unless it's the `surface_container_lowest`. It kills the "layered glass" illusion.
* **Don't** clutter the screen. If data density is high, use typography size (e.g., `label-sm`) rather than more lines or boxes to separate info.