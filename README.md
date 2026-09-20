# Taskbar Volume

A tiny Windows resident utility written in Rust. When the pointer is over the primary or secondary Windows taskbar, scrolling the mouse wheel sends the Windows volume-up or volume-down media key. This changes the default output volume and opens the native Windows 11 volume flyout. The wheel event is consumed so taskbar widgets do not react at the same time.

## Run

```powershell
cargo run --release
```

The release executable is `target\\release\\taskbar-volume.exe`. It has no console window and shows a notification-area icon as confirmation that it is running. Right-click that icon and choose **Exit** to stop it.

If it does not react, inspect `taskbar-volume.log` beside the executable. It records startup status and each wheel event without writing outside the executable folder.

## Notes

- It works with horizontal, vertical, auto-hidden, and multi-monitor taskbars as long as Windows reports the cursor's target as a taskbar window.
- It controls the system volume, not individual-app volume. The increment follows the Windows media-key volume increment.
- The program uses one system-wide low-level mouse hook and makes no periodic polling calls.
