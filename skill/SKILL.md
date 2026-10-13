---
name: wayfinder
description: Point at things on the user's Windows screen with an on-screen arrow instead of clicking them. Use when the user asks how to do something on their computer ("show me how to", "where do I click", "how do I open", "walk me through", "point at it") or asks you to use the arrow/wayfinder. The user does every click; you point, wait for their click, then point at the next step.
---

# wayfinder: you point, they click

Requires `wayfinder serve` running on the user's PC (see README). It is an accessibility tool: it must keep working where screenshots don't, and the user should never have to type.

## Loop

1. **By name first** (instant, no screenshot): `wayfinder at "Device Manager" "Step 2: click Device Manager"`
   - Finds buttons, links, menu items, list rows, tabs and fields by visible name, web page content included. Prefers the copy on the monitor the mouse is on.
   - If it's hidden but a route was learned, it points at the first step instead. The JSON has `"button": "right"` when that step is a right-click; say so in the label.
   - Not sure of the name? `wayfinder find "part of name"` lists candidates. Try a few likely words.
2. **Wait for the click, in the background** (it can take minutes): `wayfinder wait 600`. Exit 0 = clicked inside the ring. The overlay then shows "working out the next step..." automatically.
3. Point at the next step. Repeat. `wayfinder off` when finished.
4. **Fallback when `at`/`find` can't see it** (canvases, games, video, custom-drawn UIs): `wayfinder shot shot.png`, read the image, then `wayfinder point X Y "label"` in that image's pixels. The shot marks the mouse with a magenta crosshair and reports which monitor it's on.

## Rules

- Never make them type: `wayfinder clip "text"`, then point at the box with "Right-click here -> Paste".
- Keep labels short ("Step 3: click Safety Filters") and say the same step in chat.
- When they're waiting on you and no arrow is up, run `wayfinder busy` so they know you're working even if your window is covered.
- You can't draw over the Start menu, the search panel or the admin (UAC) prompt. Give those steps in words. `shot` fails while a UAC prompt is up; that's what it means.
