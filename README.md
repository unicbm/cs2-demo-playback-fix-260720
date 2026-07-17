# CS2 Demo Playback Fix

A small command-line tool that repairs one confirmed CS2 demo playback
compatibility failure: current clients rejecting legacy, correctly framed
entity message type `138` (`CEntityMessageRemoveAllDecals`).

[Technical investigation and rewrite contract](docs/TYPE138_COMPATIBILITY.md)

## Download and use on Windows

Put these two files in the same directory:

- `cs2-demo-playback-fix.exe`
- `repair-demo.bat`

Drag one or more `.dem` files onto `repair-demo.bat`. A repaired copy is
created beside each affected input:

```text
match.dem -> match_safe138.dem
```

The original demo and an existing output are never overwritten. If a demo does
not contain the strictly validated legacy message, the tool reports `CLEAN` and
does not create a redundant copy.

## Command line

```powershell
cs2-demo-playback-fix.exe <demo.dem> [demo.dem ...]
cs2-demo-playback-fix.exe --output <safe.dem> <demo.dem>
cs2-demo-playback-fix.exe --help
```

Successful repairs print a one-line report containing the output path, number
of changed frames, number of removed messages, and first/last affected ticks.

## What the tool changes

The tool performs a narrow, fail-closed rewrite:

1. Parse the `PBDEMS2` outer frame container.
2. Decompress only relevant raw Snappy packet frames.
3. Parse the Source 2 unaligned netmessage bitstream.
4. Remove type `138` only when its payload exactly matches the verified legacy
   `CEntityMessageRemoveAllDecals` schema.
5. Rebuild affected lengths and header offsets, then verify the output.

Unaffected outer frames are copied byte-for-byte. Kept netmessages retain their
original bit ranges. Removing this message may change when old decals such as
blood marks or bullet holes are cleared; it does not intentionally alter ticks,
player state, equipment, or round events.

This is not a general demo upgrader, a parser replacement, or an animation and
asset compatibility layer. A different playback failure requires a separate,
evidence-backed patch.

## Build

Install stable Rust, then run:

```powershell
cargo test --locked
cargo build --release --locked
```

The executable is written to `target\release\cs2-demo-playback-fix.exe`.
`package-windows.ps1` builds a distributable zip containing the executable,
drag-and-drop batch file, documentation, and license notices.

## License

Licensed under the [Apache License 2.0](LICENSE). The binary uses the Rust
`snap` crate under BSD-3-Clause; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
