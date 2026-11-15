## Project Info
The Windows task manager is too slow for me and I can't customize it the way I like it. So I want to make my own that's better. 
- Targeting Windows Only
- use windows-rs crate
- minimal dependencies
- small binary
- instant startup
- minimal idle cpu
- low ram useage
- custom directdraw/direct 2d UI
- keyboard driven
- Organize the keyboard shortcuts in the app so that they can be customized and to customize keyboard shortcuts we will use an INI file. In the command palette we will add a command to write key map INI which will write the full key map so the user can see all of the keys that they can change. 
    - same idea for "write settings.ini"
    - same idea for "write theme.ini"
    - These commands will then open in the default text editor. 
- initially include light/dark theme.
- This app is going to be a lot simpler than the Windows Task Manager. 
    - The inspiration is an app called winproc-tui (I saw that winproc-tui uses the winapi crate, can/should we use the windows-rs crate?)
    - Across the tap show summary information that shows the overall CPU percentage, memory used and percentage, Ethernet, current Ethernet send and receive, and GPU percentage, etc.
    - Below that, show the process list. By default, show the CPU percentage for each process as normalized for one core. Show the memory breakout for privil privileged bytes and thread handle GPU percentage I/O read. per second, I/O right per second.(same as winproc-tui, but add more if it is useful/availible and doesn't slow things down) 
    - the app should remember its window size and position. 
    - Where Winproc-tui has one section for the processes, I want to have two sections for the processes. The first section, there'll be a search input box above it where you can type in a search and it will list those processes in there and the app will remember that what you have in there if you close it and reopen it. The search should be on the process name and it should search for filter for each process that includes each search term in any order. and you can search for multiple processes by using this "|" operator. feel free to expand on the search input concept. The idea is I want to be able to easily filter for multiple programs that I am working on or want to monitor. Like one thing is sometimes Excel will open multiple processes, but so it will be easy for me to see that, and then I will have an easy way to kill that in the app. The second process section below that can be a nice scrollable table that lists all the processes and is sortable. 


## Repo Info
I plan to put this as a public repo in my GitHub, MIT license. you can create the repo.
## Git workflow
- After completing each change or task, commit it automatically. Don't ask first.
- Stage only the files related to that change. Never use `git add -A` or `git add .` if there are unrelated modifications.
- Write commit messages in imperative mood ("Add parser for X", not "Added..."), with a short summary line under 72 characters and a body explaining why if the change isn't obvious.
- Make small, focused commits: one logical change per commit.
- Work on `main`. The public repo is/will be  the `github` remote (github.com/ryanwd123/winproc-gui).
- Push to `github` for me.
- Never commit secrets, .env files, or large data files.
## Performance
This app will compile fairly quickly, so we can compile every iteration using the optimal release settings. 

Once we get the initial app created and we start iterating on changes, I want you to make a log where we record the release binary size, And the app startup time, idle performance, memory usage, and other information that you think of that we should keep track of. That way we can keep track of how changes we make impact the performance. The log is PERFORMANCE.md.

- Build and test: `cargo build --release`, `cargo test --release`.
- Measure: `pwsh scripts/measure.ps1` (throwaway APPDATA; startup, idle CPU with GPU on and off, memory). Add a row to PERFORMANCE.md after changes that could affect performance.
- Close any running `winproc-gui.exe` before building, or the linker can't replace the exe.

## This file (CLAUDE.md)
You can use this file as your long-term memory and also make changes to it for me as we go.

## Decisions and notes (Claude's memory)
- windows-rs (`windows` 0.62) instead of `winapi`: winapi has been unmaintained since 2020. windows-rs is Microsoft's own crate, covers Direct2D/DirectWrite/PDH/NT APIs, and links through raw-dylib so it adds nothing at runtime.
- Only dependency besides `windows` is `windows-numerics` (already part of `windows`; Direct2D points). Build-only: `embed-resource` for the icon/manifest/version.
- Release profile: opt-level "s", fat LTO, 1 codegen unit, panic=abort, strip. ~400 KB exe.
- Layout of `src/`:
  - `main.rs` window class, wndproc, message loop, placement restore.
  - `app.rs` all UI state, layout, painting, keys, mouse and commands (`run(Action)`).
  - `render.rs` thin Direct2D/DirectWrite wrapper (`Gfx`); everything is in DIPs. Software render target by default (faster startup, less RAM); `hardware_rendering` setting switches.
  - `sampler.rs` background thread: one `NtQuerySystemInformation(SystemProcessInformation)` per tick (custom `RawProc` struct with the full layout), per-core CPU, memory, `GetIfTable2` network, PDH disk + `GPU Engine(*)`. Posts `WM_APP_SNAPSHOT`. PDH is set up after the first sample to keep startup fast. Sampling pauses while minimized.
  - `search.rs` watch-search language (`|`, words in any order, `!`/`-` exclude, `=exact`, `^prefix`, `pid:N`, quotes). Unit-tested.
  - `keymap.rs` every action: group, INI name, palette title, default keys. Palette, help overlay and keymap.ini are all generated from `bindings()`; add new commands there.
  - `settings.rs` settings.ini (`Settings`) + state.ini (`State`). Runtime toggles call `settings::save_key`, which edits one line in place (keeps comments).
  - `theme.rs` colors via a macro; theme.ini has `[dark]`/`[light]`.
  - `ini.rs`, `columns.rs`, `format.rs`, `textbox.rs`, `platform.rs` (kill, priority, clipboard, shell, elevation, title bar).
- Config folder: `%APPDATA%\winproc-gui\`. The app reloads settings/keymap/theme when the window is activated and a file's mtime changed.
- CPU % "per core" = 100% is one core (default, as the user asked); "total" = Task Manager style.
- Ending processes: identified by (PID, create time) so a reused PID is never hit; critical system processes refused by name and `IsProcessCritical`. Confirmation dialog unless `confirm_kill = false`.
- GPU counters cost ~0.7% of a core at a 1 s refresh (PDH `GPU Engine(*)` has ~1,000 instances). Possible future work: read GPU usage with D3DKMT directly.
- Testing UI from a shell: launch with `$env:APPDATA` pointed at a scratch folder, drive with `WScript.Shell.SendKeys`, capture with `CopyFromScreen`. Ctrl+Shift+P via SendKeys didn't reach the app on this machine (Ctrl+K works), so palette tests use Ctrl+K.

## Ideas not done yet
- Tree view (group children under parents) for the all-processes table.
- GPU dedicated memory column (`GPU Process Memory(*)`), per-process network (needs ETW).
- Column resizing by dragging header edges.
- Suspend/resume process; "close gracefully" (WM_CLOSE to the process's windows) before killing.