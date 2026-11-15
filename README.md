# winproc-gui

A fast, keyboard-driven process monitor for Windows. It is meant to start instantly, use almost no CPU when idle, and make it easy to watch and end the handful of programs you care about.

- **Summary tiles:** CPU (with per-core bars), memory and commit, disk active time and throughput, network send and receive, GPU, and process, thread and handle counts. Each tile has a history graph.
- **Watch table:** type `excel | code | python` and only those processes are listed, with a totals row. The search is remembered between runs. Press Ctrl+Shift+K to end everything the watch table lists, for example all of Excel's leftover processes.
- **All-processes table:** every process, sortable by any column.
- **Columns:** name, PID, CPU % (per core or whole machine), private bytes, working set, threads, handles, GPU %, I/O read/s and write/s. You can also add parent PID, CPU time, private working set, other I/O, session, priority, age and path.
- **Customizable:** commands in the palette write `keymap.ini`, `settings.ini` and `theme.ini` out in full and open them in your editor. Save the file and switch back, and the app reloads it.
- Dark and light themes, and your window size and position are remembered.

Built with the [`windows`](https://github.com/microsoft/windows-rs) crate (Direct2D and DirectWrite for drawing) and no other runtime dependencies. The release exe is about 400 KB.

## Build

```
cargo build --release
```

The exe is `target\release\winproc-gui.exe`. It needs Windows 10 1607 or later.

## Watch search

| Search | Lists |
|---|---|
| `excel` | names containing "excel" (case-insensitive) |
| `micro edge` | names containing every word, in any order |
| `excel \| code` | either one; use as many `\|` alternatives as you like |
| `code !helper` | leaves out names containing "helper" (`-helper` works too) |
| `=code` | exactly `code` or `code.exe` |
| `^py` | names starting with "py" |
| `pid:1234` | one process ID |
| `"two words"` | a term containing a space |

In the search box, Up and Down go through earlier searches. In the all-processes table, Insert adds the selected process to the search. Alt+T also lists the child processes of watched processes.

## Keys

Press F1 in the app for the full list. Ctrl+Shift+P (or Ctrl+K) opens the command palette. Run **Write keymap.ini** there to see and change every shortcut.

| Keys | Action |
|---|---|
| Ctrl+F, `/` | focus the watch search |
| Tab / Shift+Tab | move between the search box, the watch table and the all-processes table |
| Up, Down, PgUp, PgDn, Home, End | move the selection |
| Delete | end the selected process (asks first) |
| Shift+Delete | end the process and its children |
| Ctrl+Shift+K | end every watched process |
| Insert | add the selected process to the watch search |
| Ctrl+1 … Ctrl+9 | sort by a column; press again to reverse |
| Alt+C | CPU %: per core or whole machine |
| Ctrl+T | dark / light theme |
| Ctrl+= / Ctrl+- / Ctrl+0 | font size |
| Ctrl+E | open the file location |
| Ctrl+C / Ctrl+Shift+C | copy name, PID and path / copy the path |
| F5, F9 | refresh now, pause |

## Settings

The files live in `%APPDATA%\winproc-gui\`:

- `settings.ini`: theme, font, refresh interval, columns, CPU mode, GPU sampling, kill confirmation and more.
- `keymap.ini`: every action and its keys.
- `theme.ini`: every color of the dark and light themes.
- `state.ini`: window placement, the search and its history, and sort order. The app writes this file.

Commands that change a setting (theme, font size, columns and so on) update only that line of `settings.ini` and leave the rest of the file alone.

## How it works

- One `NtQuerySystemInformation(SystemProcessInformation)` call per refresh returns every process's times, memory, handle, thread and I/O counters. A sample takes about 5 ms on a machine with 270 processes, and it runs on a background thread.
- Per-core CPU comes from `SystemProcessorPerformanceInformation`, memory from `GlobalMemoryStatusEx` and `GetPerformanceInfo`, and network from `GetIfTable2`.
- Disk and GPU come from PDH counters (`PhysicalDisk(_Total)` and `GPU Engine(*)`). The GPU counters are what Task Manager reads too, and they cost most of the app's idle CPU. Set `gpu = false` to turn them off.
- Drawing uses a software Direct2D render target: it starts faster and uses less memory than the GPU path for a window that redraws once a second. Set `hardware_rendering = true` to use the GPU instead.
- Sampling stops while the window is minimized.

See [PERFORMANCE.md](PERFORMANCE.md) for measurements.

## License

MIT
