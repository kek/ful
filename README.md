# ful & dum

Two terminal disk tools that go together:

- **ful** — a live dashboard of your mounted filesystems: usage bars,
  used/free/total, and per-device I/O. *Which disk is in trouble?*
- **dum** — an ncdu-style tree explorer that watches filesystem events and
  illuminates what's changing: directories receiving writes glow green with
  a live rate and sparkline, shrinking ones glow red, sizes update in place.
  *What exactly is moving?*

Install both with one command:

    cargo install ful

## ful

    ful                  # default 2s refresh
    ful --interval 1     # custom refresh interval (seconds)

Keys: `q`/Esc quit · `?` help · `a` toggle pseudo filesystems.

## dum

    dum                  # explore the current directory, watching live
    dum ~/src            # explore a specific path
    dum --no-watch PATH  # plain explorer, no live updates

Keys: arrows/`hjkl` move · `⏎` enter · `u`/`⌫` up · `r` rescan ·
`?` help · `q`/Esc quit.

dum is read-only: it never modifies, moves, or deletes anything.

## Notes

- Sizes are allocated (on-disk) bytes, like `du`.
- macOS reports I/O per physical device; if `sysinfo` surfaces no per-disk
  counters, ful's READ/s and WRITE/s columns show `—`.
- dum's live layer uses FSEvents on macOS (via `notify`). Hardlinks are
  counted naively. Linux/Windows are untested in v1.
- If event watching fails or overflows, dum keeps working as a plain
  explorer and shows "degraded" — press `r` to rescan.
