# CS2 Demo Playback Fix

这是一个非常小、非常克制的 CS2 Demo 播放兼容性修复工具。

它只处理一种经过实机 A/B 验证的故障：旧 Demo 中存在 framing 正确的 entity
message type `138`（`CEntityMessageRemoveAllDecals`），当前客户端播放到它时可能
显示 `Parse Message Error`、`Unknown message type 138`，随后退出 Demo。

## Windows 直接使用

把下面两个文件放在同一个目录：

- `cs2-demo-playback-fix.exe`
- `repair-demo.bat`

将一个或多个 `.dem` 文件拖到 `repair-demo.bat` 上即可。受影响的 Demo 会在原目录
旁边生成：

```text
match.dem -> match_safe138.dem
```

工具不会修改原 Demo，也不会覆盖已经存在的输出。若 Demo 不包含严格匹配的旧
type `138`，程序会显示 `CLEAN`，并且不会制造一份没有意义的副本。

## 命令行

```powershell
cs2-demo-playback-fix.exe <demo.dem> [demo.dem ...]
cs2-demo-playback-fix.exe --output <safe.dem> <demo.dem>
cs2-demo-playback-fix.exe --help
```

转换速度主要受 Demo 顺序读取和少数 Snappy frame 重压影响，通常非常快。

## 安全边界

本工具会解析 `PBDEMS2` outer frame、packet protobuf wrapper 和 Source 2 非字节
对齐 netmessage 位流。只有 type 和 payload schema 都严格符合已验证旧消息时才会
删除；其余 outer frame 逐 byte 保留，保留下来的 netmessage 也保留原始 bit range。

删除 `RemoveAllDecals` 的预期可见副作用，仅是部分旧血迹或弹孔可能不再按原时机
清理。它不应改变 tick、玩家状态、装备或回合事件。

这不是万能的“旧 Demo 升级器”，也不负责已经消失的旧人物、动画或资源。其他
兼容性故障必须使用独立 patch、严格前置条件和新的实机验证，不能扩大 type `138`
补丁的匹配范围。

完整调查与格式说明见
[docs/TYPE138_COMPATIBILITY.zh-Hans.md](docs/TYPE138_COMPATIBILITY.zh-Hans.md)。

## 从源码构建

```powershell
cargo test --locked
cargo build --release --locked
```

可执行文件位于 `target\release\cs2-demo-playback-fix.exe`。运行
`package-windows.ps1` 可以生成包含 EXE、BAT、说明和许可证的 Windows zip。

## 许可证

项目使用 [Apache License 2.0](LICENSE)。`snap` 依赖使用 BSD-3-Clause，详见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
