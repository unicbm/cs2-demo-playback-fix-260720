# CS2 Demo Message 138 Playback Compatibility

Status: confirmed by controlled before/after playback tests on affected demos.

Last validated: July 17, 2026, against the current CS2 client using demo
version `14171`.

## 1. Finding

Some CS2 demos recorded before the July 8/9, 2026 Source 2 update fail during
playback in the current client. A demo may fail immediately or play normally
until CS2 disconnects with `Failed to parse message` or reports an unknown
message type in the console.

In the affected samples examined for this project, the first fatal boundary is
a correctly framed Source 2 entity netmessage with type `138`:

```text
EM_RemoveAllDecals = 138
CEntityMessageRemoveAllDecals
```

Removing only instances whose payload exactly matches the verified legacy
schema allows the same demos to continue past their original failure points.

This establishes a playback compatibility failure at message 138. It does not
establish that Valve intentionally removed the message or its schema. The
current public GameTracking-CS2 protobuf snapshot still defines both
`EM_RemoveAllDecals = 138` and `CEntityMessageRemoveAllDecals`. The deeper
closed-client cause may be a handler-registration, dispatch, or compatibility
regression and remains unknown.

## 2. Validation evidence

Two affected samples produced these independent repair statistics:

```text
sample A
  removed messages: 281
  changed frames:    29
  first tick:        4
  last tick:         210040
  max per frame:     10

sample B
  removed messages: 200
  changed frames:    20
  first tick:        4234
  last tick:         182098
  max per frame:     10
```

For the tested files:

1. The original demo failed at or after the first affected packet.
2. The repaired demo contained no remaining strictly matching type 138
   messages.
3. An independent demo parser completed on both the original and repaired
   files with unchanged analysis results.
4. The current CS2 client played the repaired files past the original failure
   points.

Third-party parser success alone is not sufficient evidence: the defect is in
the current client playback path, not necessarily in general-purpose demo
parsers.

## 3. Container and packet layout

### 3.1 PBDEMS2 short header

The file begins with a 16-byte header:

```text
offset  size  meaning
0       8     "PBDEMS2\0"
8       4     little-endian absolute offset of DEM_FileInfo
12      4     little-endian absolute offset of DEM_SpawnGroups
```

Rewriting earlier packet frames changes later absolute positions. Both nonzero
header offsets must therefore be remapped to the corresponding new outer-frame
boundaries. Zero offsets remain zero.

Do not stop scanning at `DEM_Stop`; relevant header targets can occur after it.

### 3.2 Outer frames

Frames begin at byte offset 16:

```text
[command: varint u32]
[tick:    varint u32]
[size:    varint u32]
[payload: size bytes]
```

Command bit `64` marks a compressed payload:

```text
compressed = (raw_command & 64) != 0
command    = raw_command & ~64
```

Relevant commands are:

| Command | Value | Payload |
|---|---:|---|
| `DEM_FileInfo` | 2 | Header offset target |
| `DEM_Packet` | 7 | `CDemoPacket` |
| `DEM_SignonPacket` | 8 | `CDemoPacket` |
| `DEM_FullPacket` | 13 | `CDemoFullPacket` |
| `DEM_SpawnGroups` | 15 | Header offset target |

Compressed outer payloads use raw Snappy blocks, not Snappy framed streams.
Only changed frames are recompressed. Unchanged frames, including their
original varint encodings and compressed bytes, are copied byte-for-byte.

### 3.3 Packet protobuf wrappers

The target bitstream is stored in these length-delimited fields:

```proto
message CDemoPacket {
    optional bytes data = 3;
}

message CDemoFullPacket {
    optional CDemoStringTables string_table = 1;
    optional CDemoPacket packet = 2;
}
```

For commands 7 and 8, rewrite `CDemoPacket.field 3`. For command 13, enter
`CDemoFullPacket.field 2` and then rewrite the nested `CDemoPacket.field 3`.

The implementation operates at protobuf wire level and replaces only the
target length-delimited value and its length varint. Other fields, field order,
and unknown fields remain byte-identical. Decoding and re-encoding the entire
`CDemoFullPacket` would unnecessarily rewrite large string tables and could
drop fields unknown to the local schema.

### 3.4 Unaligned Source 2 netmessage stream

`CDemoPacket.data` is not a byte-aligned protobuf sequence. Each message is:

```text
[message type: Source 2 UBitVar]
[payload size: ordinary varint beginning at the current bit position]
[payload:      payload_size * 8 bits beginning at that bit position]
```

Bits are read least-significant-bit first. UBitVar decoding is:

```text
first = read_bits(6)
selector = first & 0x30

0x10: value = (first & 0x0f) | (read_bits(4)  << 4)
0x20: value = (first & 0x0f) | (read_bits(8)  << 4)
0x30: value = (first & 0x0f) | (read_bits(28) << 4)
else: value = first
```

Type 138 uses ten bits. In observed demos, a complete type 138 record occupied
82 or 90 bits depending on payload length. Removing it changes the alignment of
all following messages, so byte search or byte splicing is invalid.

Parsing continues while more than eight bits remain. After rewriting a changed
stream, old residual bits are discarded and the final byte is zero-padded. The
kept messages are then reparsed and compared against their original bit ranges.

## 4. Strict legacy payload predicate

The expected schema is:

```proto
message CEntityMessageRemoveAllDecals {
    optional bool remove_decals = 1;
    optional CEntityMsg entity_msg = 2;
}

message CEntityMsg {
    optional uint32 target_entity = 1 [default = 16777215];
}
```

Before removing a type 138 record, the wire payload must satisfy all of these
conditions:

1. The outer message has exactly two fields in the order field 1, field 2.
2. Field 1 uses wire type 0 and its complete varint value is `1`.
3. Field 2 uses wire type 2 and contains one complete nested message.
4. The nested message has exactly one field: field 1 with wire type 0.
5. The nested varint is fully consumed and fits losslessly in `u32`.
6. No unknown, repeated, reordered, or trailing fields are accepted.

This predicate is intentionally stricter than normal generated-protobuf
decoding. Any mismatch fails closed instead of guessing that an unfamiliar
message is safe to remove.

## 5. Safe rewrite procedure

The repair operation follows this sequence:

1. Reject identical input/output paths and existing output files.
2. Create an exclusive temporary file in the output directory.
3. Validate the `PBDEMS2` header and record both original absolute offsets.
4. Scan every outer frame through EOF while tracking old and new positions.
5. For commands 7, 8, and 13, decompress when required and locate the packet
   data through wire-level protobuf parsing.
6. Parse all unaligned netmessages and validate every selected type 138
   payload before writing any modified packet.
7. Copy the exact bit ranges of all kept messages into a new zero-padded
   stream and verify them by reparsing.
8. Rebuild only the affected protobuf lengths, Snappy block, and outer frame
   size.
9. Remap the short-header offsets to the new command 2 and command 15 frame
   boundaries.
10. Flush, sync, and rescan the completed temporary output.
11. Publish without overwriting, or delete the temporary file on any failure.

If no strictly matching message is found, the CLI reports `CLEAN`, removes the
temporary file, and does not create a redundant output copy.

## 6. Required invariants

A compatible implementation must preserve these properties:

- The source demo is never modified or overwritten.
- Unaffected outer frames remain byte-for-byte identical.
- Kept netmessages retain order, type, payload size, payload bytes, and exact
  encoded bit ranges.
- The original compressed/uncompressed flag is retained for every frame.
- Non-target protobuf fields remain byte-identical.
- Every nonzero remapped header offset points to the correct new frame
  boundary and command.
- The rewritten outer frames cover the file through EOF with no gaps or short
  reads.
- A repaired packet contains no remaining selected type 138 records.
- Running the repairer again reports `CLEAN` and does not create another file.
- Malformed varints, lengths, Snappy data, protobuf fields, bit ranges, or
  payload schemas fail closed.

Implementations should bound outer frame sizes, declared Snappy output sizes,
protobuf lengths, payload bit lengths, and all integer conversions.

## 7. Test coverage

The project includes synthetic tests for:

- non-byte-aligned removal with neighboring messages preserved;
- packet field replacement while preserving unknown protobuf fields;
- rejection of an unexpected type 138 payload;
- clean packets as no-ops;
- outer and inner `u32` varint overflow rejection.

Release validation additionally includes:

- `cargo fmt -- --check`;
- `cargo test --locked`;
- `cargo clippy --all-targets --locked -- -D warnings`;
- a Windows release build and package smoke test;
- repair and second-pass `CLEAN` verification on an affected official LAN
  demo;
- current-client playback beyond the original failure point.

## 8. Integration guidance

Applications that analyze and play demos should preserve the original file:

```text
original demo
  |-- analysis/parser path: read the original
  `-- current-client playback path: create a repaired temporary copy
```

If repaired copies are cached, use at least the source content hash, a stable
patch identifier, and a patch revision. A suitable identifier for this patch
is:

```text
drop-legacy-remove-all-decals-138
```

Do not combine unrelated compatibility fixes under a generic "repair old
demos" switch. Each future patch should have its own evidence, strict
preconditions, identifier, revision, and verification.

## 9. Scope and limitations

This patch addresses one confirmed parser failure. It does not guarantee that
all historical demos are compatible with the current client. Older demos can
also depend on removed models, animation graphs, materials, or other resources
that a packet rewrite cannot restore.

The expected visible side effect is limited to some legacy decals not being
cleared at their originally recorded time. The repair does not intentionally
change player positions, ticks, equipment, events, or round state.

If a repaired demo fails later, treat the new failure as a separate
investigation. Do not broaden the type 138 predicate without new controlled
evidence.

## 10. References

- Project implementation: [`src/main.rs`](../src/main.rs)
- GameTracking-CS2 protobuf snapshot:
  <https://github.com/SteamTracking/GameTracking-CS2/blob/master/Protobufs/usermessages.proto>
- demoparser outer-frame reader:
  <https://github.com/LaihoE/demoparser/blob/main/src/parser/src/first_pass/frameparser.rs>
- demoparser Source 2 bit reader:
  <https://github.com/LaihoE/demoparser/blob/main/src/parser/src/first_pass/read_bits.rs>
- Snappy block format:
  <https://github.com/google/snappy/blob/main/format_description.txt>
