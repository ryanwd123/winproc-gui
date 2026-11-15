# Performance log

One row per measured change, so we can see what each change costs. Measured on the
release build with `pwsh scripts/measure.ps1`, which uses a throwaway settings folder.

- **Startup:** process launch until the main window exists. Median of 7 runs (min–max). The first run is usually a cold start.
- **Idle CPU:** CPU cycles per second (`QueryProcessCycleTime`) over 20 s with the window open at 1 s refresh. Measured three ways: main view with GPU sampling on (the default), main view with it off, and the watch view watching "svchost". At about 5 GHz, 100 Mcycles/s is about 2% of one core. Runs vary by roughly ±30%, so only bigger differences mean anything.
- **Private / Working set:** memory about 25 s after startup, main view, GPU sampling on.

Machine: Intel i7-13700K (24 logical), 64 GB RAM, RTX 3090 + UHD 770, Windows 11 Pro 22631, about 260 processes. Window 1300×780.

| Version | Date | Commit | Binary size | Startup (median, min–max) | Idle, GPU on | Idle, GPU off | Idle, watch view | Private | Working set | Threads | Handles |
|---------|------|--------|-------------|---------------------------|--------------|---------------|------------------|---------|-------------|---------|---------|
| 0.1.0 | 2026-11-15 | `2464124` | 412,672 B (403 KiB) | 57.9 ms (47.4–119.6) | 141 ms CPU / 20 s* | 0 ms CPU / 20 s* | – | 39.1 MB | 48.4 MB | 41 | 300 |
| 0.1.0 | 2026-11-15 | `6e164e6` | 424,448 B (414 KiB) | 55.2 ms (41.9–102.9) | 95 Mc/s | 106 Mc/s | 102 Mc/s | 28.2 MB | 37.8 MB | 22 | 301 |
| 0.1.0 | 2026-11-15 | `bef5c7b` | 424,448 B (414 KiB) | 139.9 ms (130.7–208.6) | 49 Mc/s | 48 Mc/s | 39 Mc/s | 64.3 MB | 46.0 MB | 24 | 443 |

\* The first row measured CPU *time*, which Windows counts in 15.6 ms ticks. That turned out to be far too coarse: the same build gave anywhere from 31 to 203 ms. Later rows count cycles.

## Findings

- **Drawing dominates idle CPU, not sampling.** With updates paused (F9) the app uses 0.2 Mc/s. Per thread: the UI thread uses about 20 Mc/s, the sampler about 19 Mc/s (one NtQuerySystemInformation pass, about 3–5 ms), and the rest is spread over the software renderer's worker threads at about 5 Mc/s each.
- **The software Direct2D render target costs about 100 Mc/s at 1 redraw per second; hardware costs about 26–46 Mc/s.** How much is drawn hardly matters: full tiles with CPU/GPU bars, the compact summary with no bars, and the watch view all measured within the noise of each other. Grayscale text instead of ClearType made no difference either. The software path seems to have a fixed cost per frame for the window size.
- **Hardware rendering costs about 78 ms more startup (47 → 125 ms) and about 20–35 MB more private memory.** That is the trade-off behind the `hardware_rendering` setting. Since `bef5c7b` it is on by default (idle CPU about halved); `hardware_rendering = false` restores the faster-starting, smaller software path.
- **GPU counters:** in the first measurement they looked like most of the idle cost. With cycle counts, GPU on and off are within the noise of each other at this refresh rate. They do add about 20 threads, which PDH starts. The watch view skips them unless the GPU % column is visible.
- **The watch view** skips per-process rows, paths and the system-wide reads (per-core CPU, memory, network) for unwatched processes. Sampling was never the big cost, so its idle CPU looks like the main view's; drawing dominates both.
