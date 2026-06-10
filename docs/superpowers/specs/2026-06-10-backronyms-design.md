# Backronyms in the UI: dum = "disk usage monitor", ful = "as in …ful"

## Goal

Give both tools their name-explanation in the UI:

- **dum** is backronymed as **disk usage monitor** and shows it in its title
  bar and help overlay.
- **ful** is not an acronym but a suffix, as in beauti*ful* — and the UI plays
  on it with a **dynamic subtitle** that reacts to disk state.

## ful — dynamic subtitle

The title bar (currently ` ful — disk usage monitor`) becomes:

    ful — as in plentiful     (worst disk < 60% used)
    ful — as in watchful      (worst disk < 85% used)
    ful — as in stressful     (worst disk < 95% used)
    ful — as in dreadful      (worst disk ≥ 95% used)

- **Worst disk** = max usage percentage across **non-pseudo** filesystems,
  regardless of whether the `a` toggle currently shows pseudo filesystems.
  Pseudo filesystems (devfs etc.) sit at 100% and would false-alarm.
- If no real filesystems are present (degenerate case), fall back to
  `watchful`.
- The "…ful" word is styled with the same color the usage bar uses at that
  level, so `dreadful` reads red.
- Clap `about` changes from "TUI disk usage monitor" (that identity now
  belongs to dum) to: `live filesystem dashboard — ful, as in watchful`.

## dum — disk usage monitor

- Title bar: ` dum — disk usage monitor — /path`. On narrow terminals the
  subtitle is dropped before the path is truncated (subtitle is the least
  important element).
- Help overlay header: `dum — disk usage monitor`.
- Clap `about`: `dum — disk usage monitor, live`.

## README

Update the intro bullets for both tools so the names are explained in prose
too (dum's backronym, ful's suffix joke).

## Testing

- Unit tests for the threshold → word mapping (boundary values: 59/60,
  84/85, 94/95, 100; pseudo-only edge case).
- Render test asserting ful's subtitle appears in the title line.
- Existing dum render tests updated for the new title; new assertion for the
  help overlay header.

## Non-goals

- No behavior changes to scanning, watching, or refresh.
- No new CLI flags; the subtitle is not configurable.
