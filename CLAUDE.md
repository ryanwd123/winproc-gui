## Project Info
The Windows task manager is too slow for me and I can't customize it the way I like it. So I want to make my own that's better. 
- Targeting Windows Only
### goals
- minimal dependencies
- small binary
- instant startup
- minimal idle cpu
- low ram useage
- keyboard driven


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
  - `render.rs` thin Direct2D/DirectWrite wrapper (`Gfx`); everything is in DIPs. Hardware render target by default (user's choice, 2026-11-15: lower idle CPU over faster startup and lower RAM); `hardware_rendering = false` switches to software.
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
- Views (Ctrl+Tab, `View` in app.rs, saved as `view` in state.ini): Main and Watch. The Watch view has no summary bar or all-processes table. `App::sync_filter` sends the sampler a `Filter`, and then the sampler builds full rows (and resolves paths, lazily through `ProcInfo::path` OnceLock) only for watched processes. It skips per-core CPU, memory and network, and reads GPU counters only if the GPU column is shown. Counters are still updated for every process, so rates are right after a switch.
- A search change or switch to the Watch view triggers a "peek" (`Shared::set_filter`): a rows-only rescan that reuses each process's last rates and the last totals, so the regular 1 s cadence and the graphs aren't disturbed. Switching back to Main triggers a full sample (watch samples have no summary values).
- Idle CPU is dominated by the software Direct2D render target (~100 Mcycles/s at 1 redraw/s, a fixed cost per frame regardless of content). Hardware rendering is ~26–46 Mc/s but +78 ms startup and +20–35 MB private. See PERFORMANCE.md. Measure with cycles (`QueryProcessCycleTime`), not CPU time (15.6 ms ticks are too coarse).
- Testing UI from a shell: launch with `$env:APPDATA` pointed at a scratch folder, drive with `WScript.Shell.SendKeys`, capture with `CopyFromScreen`. Ctrl+Shift+P via SendKeys didn't reach the app on this machine (Ctrl+K works), so palette tests use Ctrl+K.

## Ideas not done yet
- Tree view (group children under parents) for the all-processes table.
- GPU dedicated memory column (`GPU Process Memory(*)`), per-process network (needs ETW).
- Column resizing by dragging header edges.
- Suspend/resume process; "close gracefully" (WM_CLOSE to the process's windows) before killing.