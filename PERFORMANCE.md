# Performance log

One row per measured change, so we can see what each change costs. Measured on the
release build with `pwsh scripts/measure.ps1`, which uses a throwaway settings folder.

- **Startup:** process launch until the main window exists. Median of 7 runs (min–max). The first run is usually a cold start.
- **Idle CPU:** CPU time used during 20 s with the window open and the default 1 s refresh, as % of one core. Windows counts CPU time in 15.6 ms ticks, so "0" means less than one tick.
- **Private / Working set:** memory after about 25 s, with GPU sampling on.
- **Sample:** the in-app "sample N ms" figure (one NtQuerySystemInformation pass plus PDH, network and memory), read from the bar above the all-processes table.

Machine: Intel i7-13700K (24 logical), 64 GB RAM, RTX 3090 + UHD 770, Windows 11 Pro 22631, about 260 processes.

| Version | Date | Commit | Binary size | Startup (median, min–max) | Idle CPU, GPU on | Idle CPU, GPU off | Private | Working set | Threads | Handles | Sample |
|---------|------|--------|-------------|---------------------------|------------------|-------------------|---------|-------------|---------|---------|--------|
| 0.1.0 | 2026-11-15 | initial | 412,672 B (403 KiB) | 57.9 ms (47.4–119.6) | 141 ms / 20 s (0.70%) | 0 ms / 20 s (0.00%) | 39.1 MB | 48.4 MB | 41 | 300 | 5–10 ms |

Notes:
- 0.1.0: almost all idle CPU comes from the PDH `GPU Engine(*)` counter, which has about 1,000 instances. PDH also starts its own threads (41 threads with GPU on, about 20 with it off). Reading GPU usage through D3DKMT directly, like System Informer does, is the obvious next step if this matters.
