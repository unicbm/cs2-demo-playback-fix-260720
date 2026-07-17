# CS2 旧 Demo Type 138 兼容性故障：根因与安全重写规范

> 状态：已通过同一 demo 修复前后的受控 A/B 播放实验确认。
>
> 最后验证：2026-07-17，当前客户端 demo version `14171`。
>
> 目标读者：需要让当前 CS2 客户端播放旧 `PBDEMS2` demo 的工具开发者。

## 1. 结论

一部分旧 CS2 demo 含有 entity message type `138`：

```text
EBaseEntityMessages::EM_RemoveAllDecals = 138
```

在已验证的当前客户端旧 demo 播放路径中，这些记录会被报告为未知 type
`138`。客户端日志与原始 bitstream 对齐共同表明：在这条失败路径上，dispatcher
没有安全地消费该消息已经声明的长度和 payload，而是从长度字段继续按“下一条
消息”解析，导致 packet bitstream 立刻失去同步，最终出现：

```text
Unknown message type 138
Error parsing message type 9 (CNETMsg_SpawnGroup_ManifestUpdate [9])
Unknown message type 1
NETWORK_DISCONNECT_MESSAGE_PARSE_ERROR
```

这里的 `9` 不是 demo 中真实存在的下一条 message type，而是 type `138`
payload 的长度。后面的 `1` 来自 payload 内容。真实的下一条消息在原始 framing
中是另一种合法消息。

安全兼容补丁是：

1. 只识别 framing 正确、payload 严格符合旧
   `CEntityMessageRemoveAllDecals` schema 的 type `138`；
2. 从 `CDemoPacket.data` 的位级消息流中删除整条 type `138`；
3. 原样保留所有其他消息的 bit range；
4. 重建 packet data、protobuf 长度、Snappy block、outer frame size；
5. 重映射 16-byte short header 中的两个绝对 frame offset；
6. 写入新文件，绝不原地修改源 demo。

经上述处理后，两份原本会被当前客户端强制踢出的官方 LAN demo 均恢复正常播放。

这里需要区分两层结论：

- **已经实证的行为级根因**：旧 demo 中严格符合既有 schema 的 type `138`
  触发当前播放路径失步；删除这些完整消息后，同一 demo 恢复播放。
- **尚未实证的客户端内部原因**：为什么当前二进制在该路径中把 type `138`
  当作 unknown。可能涉及 handler 注册、demo 版本路由或 dispatcher 实现，但闭源
  客户端内部没有足够证据支持唯一归因。

## 2. 复现实证

两份样本来自不同赛事和地图，均为官方 LAN demo，不涉及社区服务器自定义字段。

| 样本 | Demo patch | 地图 | Type 138 数量 | 受影响 frame | 首条 tick | 末条 tick | 修复前 | 修复后 |
|---|---:|---|---:|---:|---:|---:|---|---|
| A | `14160` | `de_mirage` | 281 | 29 | 4 | 210040 | 进场后立即 parse error | 正常播放 |
| B | `14165` | `de_dust2` | 200 | 20 | 4234 | 182098 | 手枪局结束约 5 秒后 parse error | 正常播放 |

补充事实：

- 样本 A 共扫描 216,325 个 packet frame；样本 B 共扫描 204,380 个。
- 481 条 type `138` 全部位于 compressed `DEM_Packet`。
- 单个 frame 最多含 10 条 type `138`。
- 样本 A 的 payload 长度分布为：9 bytes × 253、8 bytes × 28。
- 样本 B 的 payload 长度分布为：9 bytes × 180、8 bytes × 20。
- 修复后复扫，两份输出的 type `138` 数量均为 0。
- 独立 demoparser 完整解析结果保持不变：
  - 样本 A：2,162,580 rows，29 rounds；
  - 样本 B：2,043,160 rows，22 rounds。

样本 B 的手枪局 active window 为 tick 1273–3913。首条 type `138` 位于 tick
4234，恰好晚 321 ticks，即 64 tick 下约 5.02 秒，与实际崩溃时机完全吻合。

## 3. 根因链

### 3.1 原始 packet 边界

样本 A 首个触发位置的真实消息顺序为：

```text
207 (payload 22 bytes)
207 (payload 37 bytes)
207 (payload 38 bytes)
138 (payload 9 bytes)
76  (payload 188 bytes)
76  (payload 227 bytes)
...
```

客户端日志却在 `138` 后报告 message type `9` 和 `1`。

这说明客户端没有跳过 `138` 的 framing：

```text
真实数据： [type=138][length=9][payload begins with protobuf field/value...]
错误解析： [type=138][next type=9][next type derived from payload...]
```

因此，`CNETMsg_SpawnGroup_ManifestUpdate [9]` 只是串读后的假象，不是 demo
真实的 SpawnGroup 消息损坏。

### 3.2 为什么不是日期一刀切

两个 demo 都早于同一轮较大的客户端更新，但失败时机不同：

- 样本 A 在 tick 4 已经含 type `138`，所以立即失败；
- 样本 B 直到 tick 4234 才首次出现 type `138`，所以能先播放完整手枪局。

更准确的判据不是 demo 日期，而是：

> 当前客户端的播放位置是否已经走到旧 demo 中第一条 type `138`。

### 3.3 已排除的解释

- **不是 C4 结算字段。** 样本 A 在比赛开始前的 tick 4 已触发；payload
  解码为 `remove_decals=true + target_entity`。
- **至少不是这两个样本中 `delta_data` 变化造成的首个致死播放点。** 补丁没有
  decode 或重写 game-state/delta fields，只删除了完整 type `138` bit ranges，
  同一文件随即恢复播放。这不排除其他旧 demo 还存在独立的 delta/schema 问题。
- **不是社区服自定义字段。** 两份样本均为不同赛事、不同地图的官方 LAN demo。
- **不是 demo 文件整体损坏。** 两份原文件均能被独立 parser 完整解析；只删除
  type `138` 后，当前客户端即可播放。
- **不是日志中显示的 type 9 本身损坏。** 原始 framing 中 type `138` 后的真实
  message type 是 76；`9` 正好是 payload size。

## 4. 修复所需的 PBDEMS2 格式

本节只描述该兼容补丁必须处理的层，不是完整的 CS2 demo 格式规范。

### 4.1 16-byte short header

```text
offset  size  meaning
0       8     ASCII magic: "PBDEMS2\0"
8       4     little-endian u32 absolute frame offset
12      4     little-endian u32 absolute frame offset
```

在两份已验证样本中：

- `[8..12]` 精确指向尾部 `DEM_FileInfo`，command `2`；
- `[12..16]` 精确指向 `DEM_SpawnGroups`，command `15`。

实现时还应把值 `0` 视为“该 offset 未提供”的 sentinel：原样保留 `0`，不要
要求它映射到 frame boundary。非零值才必须完成下述 frame identity 重映射。

不要把这两个值当成无需更新的 metadata。前面的 compressed frame 重压后，文件
大小会改变，尾部 frame 的绝对位置也会改变。

最安全的做法不是根据总长度猜值，而是：

1. 读取两个旧 offset；
2. 遍历 outer frames 时，记录旧 offset 对应的是哪个 frame；
3. 写输出时记录同一 frame 的新 offset；
4. 最后回填 short header；
5. 验证每个非零新 offset 仍落在 frame boundary，并分别指向 command 2 和 15。

不要在 `DEM_Stop` 时停止遍历。`SpawnGroups` 和 `FileInfo` 位于其后。

### 4.2 Outer frame

从 byte offset 16 开始，每个 frame 是：

```text
[command: varint u32]
[tick:    varint u32]
[size:    varint u32]
[payload: size bytes]
```

command 的 bit `64` 是压缩标志：

```text
is_compressed = (raw_command & 64) != 0
command       = raw_command & ~64
```

与本补丁相关的 command：

| Command | 值 | Payload protobuf |
|---|---:|---|
| `DEM_Packet` | 7 | `CDemoPacket` |
| `DEM_SignonPacket` | 8 | `CDemoPacket` |
| `DEM_FullPacket` | 13 | `CDemoFullPacket` |
| `DEM_SpawnGroups` | 15 | 与 header offset 验证有关 |
| `DEM_FileInfo` | 2 | 与 header offset 验证有关 |

若 `is_compressed=true`，outer payload 是 raw Snappy block，不是 Snappy framed
stream。修改后必须使用兼容 raw block 的实现重新压缩，并保留 command 的 bit 64。

未命中 type `138` 的 frame 应逐 byte 原样复制，包括原始 command/tick/size
varint 和原始 compressed payload。不要无意义地重压整个 demo。

### 4.3 Packet protobuf wrapper

相关 protobuf schema：

```proto
message CDemoPacket {
    optional bytes data = 3;
}

message CDemoFullPacket {
    optional CDemoStringTables string_table = 1;
    optional CDemoPacket packet = 2;
}
```

目标 netmessage bitstream 位于 `CDemoPacket` 的 length-delimited field 3。

- command 7/8：直接定位 `CDemoPacket.field 3`；
- command 13：先定位 `CDemoFullPacket.field 2`，再进入嵌套的
  `CDemoPacket.field 3`。

推荐实现一个通用的 protobuf wire-level field replacement：

- 支持 wire type 0、1、2、5；
- 遇到越界、畸形 varint、group wire type 3/4 时失败；
- 目标 field 缺失时视为 clean/no-op；
- 目标 field 重复时失败；
- 只替换目标 length-delimited value 和它的 length varint；
- 其他 field、字段顺序和未知字段逐 byte 保留。

不要将整个 `CDemoFullPacket` decode 成生成类型后重新 encode。那会重写大型
string tables，并可能丢失当前 schema 未知的字段。

### 4.4 `CDemoPacket.data` 内层位流

内层消息不是普通的 byte-aligned protobuf sequence。每条消息为：

```text
[message_type: Source 2 UBitVar]
[payload_size: ordinary varint bytes, but begins at current unaligned bit]
[payload: payload_size * 8 bits, also begins at current unaligned bit]
```

所有 bit 均按 LSB-first 读取。

UBitVar 的读取规则：

```python
first = read_bits(6)
selector = first & 0x30

if selector == 0x10:
    value = (first & 0x0F) | (read_bits(4) << 4)
elif selector == 0x20:
    value = (first & 0x0F) | (read_bits(8) << 4)
elif selector == 0x30:
    value = (first & 0x0F) | (read_bits(28) << 4)
else:
    value = first
```

type `138` 使用 10 bits：

```text
6-bit prefix: (138 & 0x0F) | 0x10
4-bit suffix: 138 >> 4
```

当 payload 为 8 或 9 bytes 时，整条 type `138` 的长度分别为：

```text
10 + 8 + 8*8 = 82 bits
10 + 8 + 9*8 = 90 bits
```

两者都不是 byte-aligned。删除一条会让后续消息实际前移 82 或 90 bits，并使其
相对 byte boundary 的对齐改变 2 bits。因此不能搜索某个 `0x8A` byte，也不能
按 byte 删除“type + size + payload”。

典型 parser 循环为：

```python
while bits_remaining > 8:
    message_start = bit_position
    message_type = read_ubitvar()
    payload_size = read_varint_from_current_bit_position()
    payload_start = bit_position
    skip_bits(payload_size * 8)
    message_end = bit_position
```

循环结束时原 stream 可能剩余 `<=8` 个未消费 bits，而且这些 bits 不保证全为
0。只有修改过的 packet 才应规范化尾部：不复制旧 residual bits，拼接完所有
保留消息后直接以 0 补到完整 byte。这样新 stream 的消息末尾只会剩余 0–7 个
padding bits，不会被误读为额外消息。

### 4.5 Type 138 payload

旧 schema：

```proto
message CEntityMessageRemoveAllDecals {
    optional bool remove_decals = 1;
    optional CEntityMsg entity_msg = 2;
}

message CEntityMsg {
    optional uint32 target_entity = 1 [default = 16777215];
}
```

两份样本中的 payload 均为：

```text
08 01 12 <04|05> 08 <target_entity varint>
```

例如：

```text
08 01 12 05 08 EF 81 A5 01
08 01 12 04 08 B6 81 2A
```

含义是：

```text
field 1 (varint): remove_decals = true
field 2 (bytes):  nested CEntityMsg
  field 1 (varint uint32): target_entity
```

删除前应严格验证该 schema。若 message type 是 138，但 payload 结构、字段类型、
布尔值或 entity handle 不符合预期，应 fail closed，而不是猜测并删除。

本补丁采用的 predicate 比“生成类型能够 decode”更严格，必须在 protobuf wire
层逐项满足：

1. 外层恰好有两个 field，顺序为 field 1 后 field 2；
2. field 1 是 wire type 0，完整 varint 的值为 `1`；
3. field 2 是 wire type 2，value 是完整的嵌套 message；
4. 嵌套 message 恰好有一个 field 1，wire type 0；
5. 嵌套 varint 必须完整消费该 field，且数值能无损转换为 `u32`；
6. 不接受未知 field、重复 field、字段重排或尾随 bytes。

不要只用生成 protobuf 类型 decode 后检查两个属性；生成类型通常会容忍未知字段
和字段顺序，从而把删除条件放宽到本补丁尚未验证的消息形态。

## 5. 安全重写算法

### 5.1 总体流程

```python
def repair_demo(input_path, output_path):
    if same_resolved_path(input_path, output_path):
        raise InvalidArguments("input and output must differ")
    if output_path.exists():
        raise OutputExists(output_path)

    # 原子创建并返回本进程拥有的、已打开的临时文件；不是先猜文件名再 open。
    temp_path, temp = create_exclusive_temp_in_output_directory(output_path)
    try:
        with temp:
            with open(input_path, "rb") as reader:
                report = rewrite_open_streams(reader, temp)
                flush_and_sync(temp)
        verify_complete_output(temp_path, report)
        publish_without_overwrite(temp_path, output_path)
        return report
    except BaseException:
        # 包括 parse、Snappy、复验和发布失败；绝不遗留 partial file。
        unlink_if_exists(temp_path)
        raise


def rewrite_open_streams(reader, temp):
    header = reader.read_exact(16)
    if header[0:8] != b"PBDEMS2\x00":
        raise UnsupportedDemo("expected PBDEMS2 header")

    old_offset_a = le_u32(header[8:12])
    old_offset_b = le_u32(header[12:16])
    temp.write(header)  # 回填发生在全部 frame 写完后

    old_pos = 16
    new_pos = 16
    offset_map = {}
    report = new_report()

    while not clean_eof(reader):
        old_frame_start = old_pos
        new_frame_start = new_pos

        if old_frame_start in (old_offset_a, old_offset_b):
            offset_map[old_frame_start] = new_frame_start

        raw_command, raw_command_bytes = read_varint_with_raw_bytes(reader)
        tick, raw_tick_bytes = read_varint_with_raw_bytes(reader)
        size, raw_size_bytes = read_varint_with_raw_bytes(reader)
        payload = reader.read_exact(size)

        command = raw_command & ~64
        compressed = (raw_command & 64) != 0

        replacement = None
        if command in (7, 8, 13):
            decoded = raw_snappy_decompress(payload) if compressed else payload

            if command in (7, 8):
                patched = patch_cdemo_packet(decoded, tick, report)
            else:
                patched = patch_cdemo_full_packet(decoded, tick, report)

            if patched is not None:
                replacement = raw_snappy_compress(patched) if compressed else patched

        if replacement is None:
            # 未修改 frame 必须完全按原始 bytes 复制。
            temp.write(raw_command_bytes)
            temp.write(raw_tick_bytes)
            temp.write(raw_size_bytes)
            temp.write(payload)
            new_frame_size = (
                len(raw_command_bytes) + len(raw_tick_bytes)
                + len(raw_size_bytes) + len(payload)
            )
        else:
            new_size_bytes = encode_varint(len(replacement))
            temp.write(raw_command_bytes)  # 保留压缩 bit
            temp.write(raw_tick_bytes)
            temp.write(new_size_bytes)
            temp.write(replacement)
            new_frame_size = (
                len(raw_command_bytes) + len(raw_tick_bytes)
                + len(new_size_bytes) + len(replacement)
            )

        old_pos += (
            len(raw_command_bytes) + len(raw_tick_bytes)
            + len(raw_size_bytes) + len(payload)
        )
        new_pos += new_frame_size

    new_offset_a = 0 if old_offset_a == 0 else offset_map[old_offset_a]
    new_offset_b = 0 if old_offset_b == 0 else offset_map[old_offset_b]
    if new_offset_a > U32_MAX or new_offset_b > U32_MAX:
        raise DemoTooLarge("short-header frame offset exceeds u32")
    patch_le_u32(temp, 8, new_offset_a)
    patch_le_u32(temp, 12, new_offset_b)
    return report
```

### 5.2 位流删除

```python
def strip_remove_all_decals_138(packet_data, tick, report):
    records = []
    reader = LsbBitReader(packet_data)

    while reader.bits_remaining > 8:
        start = reader.position
        message_type = reader.read_ubitvar()
        payload_size = reader.read_varint()
        payload_start = reader.position
        reader.skip(payload_size * 8)
        end = reader.position

        records.append({
            "type": message_type,
            "size": payload_size,
            "start": start,
            "payload_start": payload_start,
            "end": end,
        })

    targets = [record for record in records if record["type"] == 138]
    if not targets:
        return None  # clean; caller copies original frame bytes

    for target in targets:
        payload = read_unaligned_bytes(
            packet_data,
            target["payload_start"],
            target["size"],
        )
        validate_remove_all_decals_payload(payload)

    writer = LsbBitWriter()
    for record in records:
        if record["type"] != 138:
            writer.copy_exact_bits(
                packet_data,
                record["start"],
                record["end"],
            )

    # 不复制原 residual bits；最后一个 byte 的未使用高位保持为 0。
    output = writer.to_zero_padded_bytes()

    verify_kept_records_bit_exact(packet_data, records, output)

    # 只有 payload 校验和重写复验都成功后才计入报告。
    removed = len(targets)
    report.removed_messages += removed
    report.changed_frames += 1
    if report.first_tick is None:
        report.first_tick = tick
    report.last_tick = tick
    report.max_per_frame = max(report.max_per_frame, removed)
    return output
```

### 5.3 Protobuf 外科替换

```python
def patch_cdemo_packet(packet_proto, tick, report):
    return replace_unique_length_delimited_field(
        packet_proto,
        field_number=3,
        transform=lambda data: strip_remove_all_decals_138(data, tick, report),
    )


def patch_cdemo_full_packet(full_proto, tick, report):
    return replace_unique_length_delimited_field(
        full_proto,
        field_number=2,
        transform=lambda packet: patch_cdemo_packet(packet, tick, report),
    )
```

`replace_unique_length_delimited_field` 应复制到目标 field key **末尾**为止的所有
原始 bytes（必须包含并保留原 key），只重新写该 field 的 length varint 和 value，
再复制目标 value 之后的所有原始 bytes。不要重编码其他 protobuf fields。

## 6. 必须保持的不变量

实现必须逐项满足：

1. 源 demo 永远不被修改或覆盖。
2. 未命中 type `138` 的 outer frame byte-for-byte 不变。
3. 修改过的 frame 中，删除 138 后的每条消息在以下维度完全不变：
   - 顺序；
   - message type；
   - payload size；
   - payload bytes；
   - 最好连原始 type/size 编码 bit range 也完全不变。
4. 原 frame 的 Snappy 压缩标志保持不变。
5. `CDemoPacket` 和 `CDemoFullPacket` 的非目标 protobuf fields 原样保留。
6. short header 的每个非零绝对 offset 指向正确的新 frame boundary；零值保持为零。
7. 输出 outer frames 必须无缺口地覆盖到 EOF；任何 short read 都是错误。
8. 输出复扫不得再含符合本 patch schema 的 type `138`。
9. 修复后的 demo 再运行一次应得到 clean/no-op，不应二次改变文件。
10. 任何畸形 varint、越界长度、意外 payload schema、重复目标 field 都应
    fail closed，并删除未发布的 partial file。

实现还应限制：

- outer frame size；
- Snappy 声明的 decompressed size；
- protobuf length；
- bitstream payload size；
- short-header offset 必须能无损写入 `u32`；
- u32/u64 varint 溢出。

## 7. 验证清单

### 7.1 合成测试

至少覆盖：

- `[207, 138(size=9), 76]` 的非 byte-aligned 删除；
- 8-byte 和 9-byte 两种 138 payload；
- 满足严格 predicate、但长度不是 8/9 bytes 的合法 138 payload；
- 同一 packet 多条 138；
- compressed/uncompressed `DEM_Packet`；
- `DEM_SignonPacket`；
- `DEM_FullPacket.field2.field3`；
- packet 无 138 时整个 frame byte-identical；
- 138 payload 非预期时拒绝；
- protobuf 未知字段保留；
- header offset 重映射后仍指向 command 2/15；
- short-header offset 为 0 时保持为 0；
- 截断 frame、溢出 varint、Snappy bomb 被拒绝；
- 输出已存在时拒绝覆盖；
- `scan(repair(x)) == clean`。

### 7.2 真实样本验收

若使用与本调查相同的两份样本，统计必须为：

```text
sample A:
  removed_messages = 281
  changed_frames = 29
  first_tick = 4
  last_tick = 210040
  max_per_frame = 10

sample B:
  removed_messages = 200
  changed_frames = 20
  first_tick = 4234
  last_tick = 182098
  max_per_frame = 10
```

之后进行三层验证：

1. 修复器复扫：remaining type 138 = 0；
2. 独立 demo parser 完整解析，rows/rounds 与原文件相同；
3. 当前 CS2 客户端播放：
   - 样本 A 越过 tick 4；
   - 样本 B 越过 tick 4234，并继续播放数分钟。

第 3 层是最终验收。第三方 parser 能读，不代表当前客户端的兼容路径正确。

## 8. 给 Python 应用的推荐接入点

如果应用同时做 demo 分析和 CS2 播放，不要替换资料库中的原 demo，也不要把修复
结果写回数据库的源路径。

推荐控制路径：

```text
原 demo
  ├─> demoparser/分析：继续读取原文件
  └─> 准备当前客户端播放副本
        ├─ clean：普通复制到临时播放路径
        └─ affected：重写为 safe 临时副本
              └─> playdemo / 录制 / 播放结束后清理
```

推荐接口：

```python
report = prepare_cs2_playback_demo(
    source_path=<input.dem>,
    destination_path=<temporary-playback.dem>,
    patch_id="drop-legacy-remove-all-decals-138",
    patch_revision=1,
)
```

报告至少包含：

```json
{
  "schema_version": 1,
  "outcome": "repaired",
  "patch_id": "drop-legacy-remove-all-decals-138",
  "patch_revision": 1,
  "removed_messages": 281,
  "changed_frames": 29,
  "first_tick": 4,
  "last_tick": 210040,
  "remaining_selected_messages": 0
}
```

如果需要缓存 safe 副本，cache key 应至少包含：

```text
source content hash + patch ID + patch revision
```

不要只使用文件名。未来增加其他兼容 patch 时，按稳定 patch ID 分开统计和验证，
不要把所有“旧 demo 修复”混成不可追踪的启发式改写。

本仓库的 CLI 在找不到匹配 type `138` 时会删除临时输出，打印 `CLEAN` 并以成功
状态退出。若接入方需要 JSON、内容哈希缓存或“clean 时复制源文件”的行为，应在
自己的 wrapper 中实现；不要把 CLI 的人类可读输出当作稳定机器协议。

## 9. 常见错误实现

### 错误 1：搜索 byte `0x8A`

type 使用 UBitVar，并且常从非 byte-aligned bit position 开始。不存在稳定的
`0x8A` byte signature。

### 错误 2：删除若干 bytes

在两份已验证样本中，138 整条消息为 82 或 90 bits；这不是通用常量。实现必须按
bitstream 中声明的 payload size 计算边界，不能硬编码 8/9-byte payload 或
82/90-bit 长度。任何 byte splice 都会直接制造新的串读。

### 错误 3：只删第一条

两个样本分别有 281 和 200 条。只删第一条只会把崩溃推迟到下一条。

### 错误 4：重编码所有 netmessages

没有必要，也会扩大风险面。保留消息应复制原始 bit ranges。

### 错误 5：重新 encode 整个 `CDemoFullPacket`

可能重排或丢弃未知 protobuf fields，并无意义地重写大型 string table。

### 错误 6：忘记 Snappy、outer size 或 short header offset

内层修复正确并不代表文件容器仍正确。四层长度/位置都必须同步：

```text
netmessage bitstream
→ CDemoPacket field length
→ Snappy compressed block / outer payload size
→ short-header absolute offsets
```

### 错误 7：原地修改

任何 crash、断电或逻辑错误都可能毁掉唯一源 demo。只允许新建输出并原子发布。

## 10. 适用范围与后续故障

这个 patch 已证明 type `138` 是两份样本的第一个致死兼容点，但不承诺所有旧
demo 只有这一种不兼容消息。

如果修复后在更晚位置出现新的 parse error：

1. 保留新的完整控制台尾部；
2. 找到崩溃前最后一个真实、正确 framing 的 message；
3. 区分“客户端未知消息未跳过”与“已知消息 schema 改变”；
4. 为新问题定义独立 patch ID、严格 precondition 和独立验证；
5. 不要扩大 type 138 patch 的匹配范围。

删除 `EM_RemoveAllDecals` 的预期可见副作用仅是部分旧血迹、弹孔等 decal 可能
不会在原定时机清除。它不应改变玩家位置、tick、事件、装备或回合状态。

## 11. 参考实现与协议来源

- 本仓库已实机验证的实现：[`src/main.rs`](../src/main.rs)
- demoparser Outer frame 读取路径：
  [`frameparser.rs`](https://github.com/LaihoE/demoparser/blob/main/src/parser/src/first_pass/frameparser.rs)
- demoparser Source 2 UBitVar/varint 读取路径：
  [`read_bits.rs`](https://github.com/LaihoE/demoparser/blob/main/src/parser/src/first_pass/read_bits.rs)
- demoparser protobuf snapshot：
  [`protobuf.rs`](https://github.com/LaihoE/demoparser/blob/main/src/csgoproto/src/protobuf.rs)
- 当前 GameTracking-CS2 protobuf snapshot：
  <https://github.com/SteamTracking/GameTracking-CS2/blob/master/Protobufs/usermessages.proto>
- Snappy raw block format：
  <https://github.com/google/snappy/blob/main/format_description.txt>

截至 2026-07-17，当前 GameTracking snapshot 与 demoparser snapshot **都**包含
type `138` 及其 `CEntityMessageRemoveAllDecals` schema：

```text
136 EM_PlayJingle
137 EM_ScreenOverlay
138 EM_RemoveAllDecals
139 EM_PropagateForce
140 EM_DoSpark
141 EM_FixAngle
```

因此，公开 proto snapshot 只能证明这条消息的编号和 payload schema，不能解释
为什么当前闭源客户端的旧 demo 播放路径仍报告 `Unknown message type 138`。这个
兼容性故障的直接证据是运行时日志、原始 packet framing，以及“只删除严格匹配
的 type 138 后同一 demo 恢复播放”的受控 A/B 结果；更深一层的客户端内部原因
应保持为推断，不应写成“当前 enum 已删除 138”。
