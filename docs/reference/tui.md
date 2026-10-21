# TUI reference

Every key, pane and state in `chatgpt tui`. Press `?` inside the TUI for the same key list.

```sh
chatgpt tui
```

The TUI reads the local index, so run `chatgpt sync` first. Suggestions come from `chatgpt classify`; unjudged chats show `·`. Run `chatgpt titles` to generate local display titles without changing the order of chats in the ChatGPT app.

## Layout

| Area | Shows |
|---|---|
| Header | `<shown> of <total>`, archived or active, suggestion filter, topic, and any age, brainstorm or title filters. Right side: `marked: N delete, M archive (x to apply)` |
| Left pane: Conversations | One row per chat: mark (`D`/`A`), last-updated date, suggestion (`?` when unsure), topic or `idea:<kind>` for brainstorms, local display title if available |
| Right pane: preview | Dates and flags, Jev or Luna verdict and reason, the summary (for chats over ~12k tokens), then the transcript |
| Footer | Key hints, or the status of the last action, and one line per failure of the last apply |

## Keys

### Move

| Key | Action |
|---|---|
| `j` / `↓` | Next chat |
| `k` / `↑` | Previous chat |
| `g` / `G` | First / last chat |
| `ctrl-d` / `pagedown` | Down a page |
| `ctrl-u` / `pageup` | Up a page |
| `J` / `K` | Scroll the preview down / up |
| `space` | Scroll the preview down a page |

### Filter

| Key | Action |
|---|---|
| `/` | Filter by title. `enter` keeps the filter, `esc` clears it |
| `esc` | Clear the title filter |
| `1` | All suggestions |
| `2` | Delete suggestions |
| `3` | Archive suggestions |
| `4` | Keep suggestions |
| `5` | Unjudged chats |
| `6` | Still unsure after the available classification passes |
| `t` / `T` | Next / previous topic |
| `y` / `Y` | Older than: any, 30d, 6m, 1y, 2y, 3y |
| `b` / `B` | Brainstorms: off, any, writing, sermon, product, other |
| `A` | Switch between active and archived chats |

Filters combine: `2` then `y` twice shows delete suggestions older than six months.

### Mark and apply

| Key | Action |
|---|---|
| `d` | Mark for delete; press again to unmark |
| `a` | Mark for archive; press again to unmark |
| `u` | Unmark |
| `enter` | Take Jev's suggestion (delete or archive marks it, keep unmarks it) and move to the next chat |
| `x` | Apply all marks |

`x` opens a dialog: `Archive <n> and permanently delete <m> conversation(s). Deletes cannot be undone.` Type `apply` and press `enter` to go ahead; `esc` or anything else cancels. Nothing changes on ChatGPT before that.

### When an apply fails

When ChatGPT refuses a chat, the apply still goes through for the others:

- The chats that worked are unmarked and gone from the active list.
- **The chats that failed keep their mark**, so the list still shows them as marked and `x` retries exactly those, without marking anything again.
- The footer says how many failed (`Applied 2 change(s); 1 failed.`) and names every failure on its own line, so a failed chat stays identifiable even when a filter hides it from the list. The lines stay until the next apply.

The footer grows to hold the failures, up to half the screen; when there are more failures than fit, the last line says how many are hidden. Those are still marked, and the header's mark count says how many.

If the daemon itself can't be reached, nothing is known about which chats changed, so every mark stays and the footer says what went wrong.

### Other

| Key | Action |
|---|---|
| `c` | Copy the selected chat's transcript as markdown |
| `o` | Open the selected chat in your browser |
| `r` | Reload from the local index, for example after `chatgpt classify` in another terminal |
| `n` | Edit the selected chat's local display title; `enter` saves and `esc` cancels. ChatGPT's own title is unchanged |
| `?` | Show the key list; any key closes it |
| `q` | Quit. With unapplied marks, press `q` twice |
| `ctrl-c` | Quit immediately, discarding marks |

## Transcripts

Transcripts cached by `classify` appear immediately. Others are fetched when you select the chat, after a short pause so scrolling past doesn't trigger fetches. Fetched transcripts are cached for next time.

## Limits

- The TUI doesn't classify. Run `chatgpt classify` in another terminal and press `r`. Background syncs and the daemon's background Jev show up by themselves within about 30 seconds (not while a box is open); the cursor stays on its chat.
- Marks aren't saved when you quit.
- An age filter the CLI can't read (`3x`, or a count too large for a date) shows no chats at all, never every chat, and the footer says why. The `y` key only offers ages that read.
