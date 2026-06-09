# ful

A terminal disk-usage monitor: a live, refreshing dashboard of mounted
filesystems — usage bar, used/free/total, and per-device I/O.

## Usage

    cargo run                 # default 2s refresh
    cargo run -- --interval 1 # custom refresh interval (seconds)

Keys: `q`/Esc quit · `?` help · `a` toggle pseudo filesystems.

The table is responsive and fits within 80 columns, dropping lower-priority
columns (DEV, FS, USED, then I/O) as the terminal narrows.

## Notes

macOS reports I/O per physical device. If `sysinfo` surfaces no per-disk
counters on your system, the READ/s and WRITE/s columns show `—`.
