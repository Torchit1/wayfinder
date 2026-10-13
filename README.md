<p align="center"><img src="assets/logo.png" width="128" alt="Wayfinder logo: a mouse cursor on a red bullseye"></p>

<h1 align="center">Wayfinder</h1>

**Let an AI agent point at things on your Windows screen instead of clicking them.**

You ask your agent (Claude Code, Codex, anything that can run a command) how to do something. It puts a big red arrow and a short instruction on the exact button, link or menu item. **You** do the click. The ring flashes green, a small "working out the next step" badge appears, and the arrow moves to the next thing.

It was built as an **accessibility tool**: for people who find screens hard to navigate and want guidance, not an agent taking over their mouse.

- **Single `.exe` under 256 KB**, with a tray icon (right-click: hide arrow / quit), no runtime, no dependencies beyond Windows itself.
- **Finds controls by name, not by screenshot.** It reads the same accessibility tree screen readers use (UI Automation), so it works on sites and apps that block screen capture. Lookups take well under a millisecond from a live in-memory index.
- **Never clicks or types for you.** It only draws, watches for your click inside the ring, and can put text on your clipboard so you can right-click → Paste instead of typing.
- **Learns how to reach hidden things.** When a click opens a menu, it remembers that the menu's items are reached through that click. Next time it points at the first step. If a route stops working (an app changed), it notices and forgets it.
- **Remembers spots it can't see.** When the agent points at something from a screenshot, `--remember "name"` saves it relative to the nearest named control, so `at "name"` finds it instantly next time, even after the window moves, resizes or reflows.
- **Multi-monitor aware.** It prefers the copy of a control on the screen your mouse is on.
- **Screenshot fallback** for things with no accessibility info (canvases, games, video): the agent takes a shot, reads it, and points by coordinates.

## Quick start

```
wayfinder serve                       # run once (e.g. at login); localhost-only by default
wayfinder at "File Explorer" "Click the yellow folder"
wayfinder wait 600                    # blocks until you click inside the ring (exit 0) or timeout (1)
```

| Command | What it does |
|---|---|
| `wayfinder serve [--port N] [--lan --allow IP,IP]` | Run the overlay, index and API. |
| `wayfinder at "name" ["label"]` | Arrow on the best visible control with that name. If it's hidden and a route was learned, arrow on the first step. |
| `wayfinder find "name"` | List visible controls whose name contains this (JSON). |
| `wayfinder shot [file.png] [--width N]` | Screenshot of the whole desktop with the mouse marked; prints which monitor the mouse is on. |
| `wayfinder point X Y ["label"] [--raw] [--remember "name"]` | Arrow at X,Y in the last shot's pixels (`--raw`: real screen pixels). `--remember` saves the spot so `at "name"` finds it next time. |
| `wayfinder wait [seconds]` | Wait for the person to click inside the ring. |
| `wayfinder busy ["message"]` | Show the "working out the next step" badge (also shown automatically after each click). |
| `wayfinder clip "text"` | Put text on the clipboard (write-only; it never reads it). |
| `wayfinder off` / `quit` / `health` | Hide the arrow / stop the server / status. |

## Using it from an agent

Copy [`skill/SKILL.md`](skill/SKILL.md) into your agent's skills folder (for Claude Code: `~/.claude/skills/wayfinder/SKILL.md`). It tells the agent the loop: name lookup first, screenshot as fallback, wait for the click, never make the person type.

## Running at login

Task Scheduler → Create Task → *At log on*, action `wayfinder.exe serve`, *Run only when user is logged on*. It needs to run in your desktop session to draw on screen. It does not need admin.

## Remote agents (optional)

`wayfinder serve --lan --allow 192.168.1.50` also listens on your network, but only answers the listed IPs, and every request needs the token in `token.txt` next to `wayfinder.exe`. [`clients/wayfinder.sh`](clients/wayfinder.sh) is a curl client for an agent on another machine (`WAYFINDER_HOST`, `WAYFINDER_TOKEN`).

**Security:** anything holding the token can take screenshots of your desktop and put text on your clipboard. Keep it localhost-only unless you need remote access, allow-list exact IPs, and treat the token like a password. The API can't click, type, run programs or read the clipboard.

## Limits

- It can't draw over the Windows Start menu or search panel, or on the admin (UAC) prompt's secure desktop. Windows reserves those layers. Screenshots also fail while a UAC prompt is up, and the API reports that.
- Without admin rights, the name index can't see inside apps running as administrator. Use the screenshot fallback, or run `serve` elevated if you accept that trade-off.
- Click detection is a 45 px circle around the target.

## Building

```
cargo build --release      # target/release/wayfinder.exe
```

The release profile is tuned for size (`opt-level = "z"`, LTO, `panic = "abort"`, stripped). To stay small, the JSON, HTTP and PNG handling are hand-written or use Windows' built-ins (WIC for PNG) rather than crates.

## Licence

MIT
