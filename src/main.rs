//! Narrow CS2 demo playback compatibility repair for legacy entity message 138.
//!
//! The rewriter preserves every unaffected outer frame byte-for-byte, validates
//! the legacy payload schema before removal, and never overwrites an input or
//! existing output file.

use snap::raw::{decompress_len, Decoder as SnapDecoder, Encoder as SnapEncoder};
use std::env;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const DEMO_HEADER_LEN: usize = 16;
const DEMO_MAGIC: &[u8; 8] = b"PBDEMS2\0";
const DEM_PACKET: u32 = 7;
const DEM_SIGNON_PACKET: u32 = 8;
const DEM_FULL_PACKET: u32 = 13;
const DEM_IS_COMPRESSED: u32 = 64;
const REMOVE_ALL_DECALS: u32 = 138;
const MAX_FRAME_SIZE: usize = 512 * 1024 * 1024;

type Result<T> = std::result::Result<T, PlaybackFixError>;

#[derive(Debug)]
struct PlaybackFixError(String);

impl fmt::Display for PlaybackFixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PlaybackFixError {}

impl From<io::Error> for PlaybackFixError {
    fn from(value: io::Error) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Default)]
struct RewriteStats {
    outer_frames: u64,
    packet_frames: u64,
    changed_frames: u64,
    removed_messages: u64,
    first_removed_tick: Option<u32>,
    last_removed_tick: Option<u32>,
    max_removed_in_frame: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetMessage {
    message_type: u32,
    payload_size: usize,
    start_bit: usize,
    payload_start_bit: usize,
    end_bit: usize,
}

#[derive(Debug, Clone, Copy)]
struct ProtoField {
    number: u64,
    wire_type: u8,
    key_end: usize,
    value_start: usize,
    end: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cs2-demo-playback-fix: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if arguments.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_help();
        return Ok(());
    }
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("cs2-demo-playback-fix {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let (inputs, explicit_output) = parse_arguments(arguments)?;
    for input in inputs {
        let output = match &explicit_output {
            Some(output) => output.clone(),
            None => default_output_path(&input)?,
        };
        match rewrite_demo_file(&input, &output)? {
            Some(stats) => println!(
                "REPAIRED input={} output={} frames={} packet_frames={} changed_frames={} removed={} first_tick={} last_tick={} max_per_frame={}",
                input.display(),
                output.display(),
                stats.outer_frames,
                stats.packet_frames,
                stats.changed_frames,
                stats.removed_messages,
                display_tick(stats.first_removed_tick),
                display_tick(stats.last_removed_tick),
                stats.max_removed_in_frame,
            ),
            None => println!(
                "CLEAN input={} no matching legacy message 138 found; no output written",
                input.display()
            ),
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "Repair the legacy entity-message-138 playback incompatibility in PBDEMS2 demos.\n\n\
         Usage:\n  cs2-demo-playback-fix <demo.dem> [demo.dem ...]\n\
         \n  cs2-demo-playback-fix --output <safe.dem> <demo.dem>\n\n\
         Without --output, each result is written beside its input as *_safe138.dem.\n\
         Existing outputs and input files are never overwritten.\n\
         A clean demo produces no output file.\n\n\
         Options:\n  -h, --help       Show this help\n  --version        Show the version"
    );
}

fn parse_arguments(arguments: Vec<std::ffi::OsString>) -> Result<(Vec<PathBuf>, Option<PathBuf>)> {
    let mut inputs = Vec::new();
    let mut output = None;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--output" {
            index += 1;
            let value = arguments
                .get(index)
                .ok_or_else(|| fail("--output requires a path"))?;
            if output.replace(PathBuf::from(value)).is_some() {
                return Err(fail("--output may only be specified once"));
            }
        } else if arguments[index].to_string_lossy().starts_with('-') {
            return Err(fail(format!(
                "unknown option: {}",
                arguments[index].to_string_lossy()
            )));
        } else {
            inputs.push(PathBuf::from(&arguments[index]));
        }
        index += 1;
    }

    if inputs.is_empty() {
        return Err(fail("no input demos supplied; use --help for usage"));
    }
    if output.is_some() && inputs.len() != 1 {
        return Err(fail("--output can only be used with one input demo"));
    }
    Ok((inputs, output))
}

fn default_output_path(input: &Path) -> Result<PathBuf> {
    let stem = input
        .file_stem()
        .ok_or_else(|| fail(format!("input has no file stem: {}", input.display())))?;
    let mut name = stem.to_os_string();
    name.push("_safe138.dem");
    Ok(input.with_file_name(name))
}

fn display_tick(tick: Option<u32>) -> String {
    tick.map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn rewrite_demo_file(input: &Path, output: &Path) -> Result<Option<RewriteStats>> {
    if input == output {
        return Err(fail("input and output paths must differ"));
    }
    if output.exists() {
        return Err(fail(format!(
            "refusing to overwrite existing output: {}",
            output.display()
        )));
    }
    if !input.is_file() {
        return Err(fail(format!("input is not a file: {}", input.display())));
    }

    let output_name = output
        .file_name()
        .ok_or_else(|| fail(format!("output has no file name: {}", output.display())))?;
    let mut temporary_name = output_name.to_os_string();
    temporary_name.push(format!(".{}.partial", std::process::id()));
    let temporary = output.with_file_name(temporary_name);
    if temporary.exists() {
        return Err(fail(format!(
            "temporary output already exists: {}",
            temporary.display()
        )));
    }

    let result = rewrite_to_temporary(input, &temporary);
    match result {
        Ok(stats) => {
            if stats.removed_messages == 0 {
                let _ = fs::remove_file(&temporary);
                return Ok(None);
            }
            if let Err(error) = fs::hard_link(&temporary, output) {
                let _ = fs::remove_file(&temporary);
                return Err(fail(format!(
                    "failed to publish {} as {} without overwriting: {error}",
                    temporary.display(),
                    output.display()
                )));
            }
            if let Err(error) = fs::remove_file(&temporary) {
                eprintln!(
                    "cs2-demo-playback-fix: warning: output is valid, but temporary hard link {} could not be removed: {error}",
                    temporary.display()
                );
            }
            Ok(Some(stats))
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn rewrite_to_temporary(input: &Path, temporary: &Path) -> Result<RewriteStats> {
    let input_file = File::open(input)
        .map_err(|error| fail(format!("failed to open {}: {error}", input.display())))?;
    let output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)
        .map_err(|error| fail(format!("failed to create {}: {error}", temporary.display())))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, input_file);
    let mut writer = BufWriter::with_capacity(1024 * 1024, output_file);

    let mut header = [0_u8; DEMO_HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|error| fail(format!("failed to read demo header: {error}")))?;
    if &header[..DEMO_MAGIC.len()] != DEMO_MAGIC {
        return Err(fail("input is not a PBDEMS2 demo"));
    }
    let old_file_info_offset = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let old_spawn_groups_offset = u32::from_le_bytes(header[12..16].try_into().unwrap());
    writer.write_all(&header)?;

    let mut input_offset = DEMO_HEADER_LEN as u64;
    let mut output_offset = DEMO_HEADER_LEN as u64;
    let mut new_file_info_offset = None;
    let mut new_spawn_groups_offset = None;
    let mut stats = RewriteStats::default();

    loop {
        let frame_input_offset = input_offset;
        let frame_output_offset = output_offset;
        if frame_input_offset == u64::from(old_file_info_offset) {
            new_file_info_offset = Some(to_u32_offset(frame_output_offset)?);
        }
        if frame_input_offset == u64::from(old_spawn_groups_offset) {
            new_spawn_groups_offset = Some(to_u32_offset(frame_output_offset)?);
        }

        let Some((raw_command, command_bytes)) = read_outer_varint(&mut reader, true)? else {
            break;
        };
        let (tick, tick_bytes) = read_outer_varint(&mut reader, false)?
            .ok_or_else(|| fail("demo ended while reading frame tick"))?;
        let (frame_size_u32, size_bytes) = read_outer_varint(&mut reader, false)?
            .ok_or_else(|| fail("demo ended while reading frame size"))?;
        let frame_size = usize::try_from(frame_size_u32)
            .map_err(|_| fail("frame size does not fit in memory"))?;
        if frame_size > MAX_FRAME_SIZE {
            return Err(fail(format!(
                "frame at offset {frame_input_offset} is unreasonably large: {frame_size} bytes"
            )));
        }

        let mut frame_payload = vec![0_u8; frame_size];
        reader.read_exact(&mut frame_payload).map_err(|error| {
            fail(format!(
                "demo ended inside frame at offset {frame_input_offset}: {error}"
            ))
        })?;

        let command = raw_command & !DEM_IS_COMPRESSED;
        let is_compressed = raw_command & DEM_IS_COMPRESSED != 0;
        if frame_input_offset == u64::from(old_file_info_offset) && command != 2 {
            return Err(fail(format!(
                "demo header FileInfo offset points to command {command}, expected 2"
            )));
        }
        if frame_input_offset == u64::from(old_spawn_groups_offset) && command != 15 {
            return Err(fail(format!(
                "demo header SpawnGroups offset points to command {command}, expected 15"
            )));
        }
        let rewritten = if matches!(command, DEM_PACKET | DEM_SIGNON_PACKET | DEM_FULL_PACKET) {
            stats.packet_frames += 1;
            let decoded = if is_compressed {
                let decoded_size = decompress_len(&frame_payload).map_err(|error| {
                    fail(format!(
                        "invalid Snappy length at tick {tick}, offset {frame_input_offset}: {error}"
                    ))
                })?;
                if decoded_size > MAX_FRAME_SIZE {
                    return Err(fail(format!(
                        "decompressed frame at tick {tick} is unreasonably large: {decoded_size} bytes"
                    )));
                }
                SnapDecoder::new().decompress_vec(&frame_payload).map_err(|error| {
                    fail(format!(
                        "Snappy decompression failed at tick {tick}, offset {frame_input_offset}: {error}"
                    ))
                })?
            } else {
                frame_payload.clone()
            };
            let patched = match command {
                DEM_PACKET | DEM_SIGNON_PACKET => {
                    rewrite_packet_protobuf(&decoded, tick, &mut stats)?
                }
                DEM_FULL_PACKET => rewrite_full_packet_protobuf(&decoded, tick, &mut stats)?,
                _ => unreachable!(),
            };
            match patched {
                Some(patched) if is_compressed => {
                    Some(SnapEncoder::new().compress_vec(&patched).map_err(|error| {
                        fail(format!("Snappy compression failed at tick {tick}: {error}"))
                    })?)
                }
                Some(patched) => Some(patched),
                None => None,
            }
        } else {
            None
        };

        writer.write_all(&command_bytes)?;
        writer.write_all(&tick_bytes)?;
        let written_size_len;
        let written_payload_len;
        if let Some(rewritten) = rewritten {
            if rewritten.len() > MAX_FRAME_SIZE || u32::try_from(rewritten.len()).is_err() {
                return Err(fail(format!(
                    "rewritten frame at tick {tick} is unreasonably large: {} bytes",
                    rewritten.len()
                )));
            }
            let mut rewritten_size = Vec::with_capacity(5);
            write_varint(&mut rewritten_size, rewritten.len() as u64);
            writer.write_all(&rewritten_size)?;
            writer.write_all(&rewritten)?;
            written_size_len = rewritten_size.len();
            written_payload_len = rewritten.len();
        } else {
            writer.write_all(&size_bytes)?;
            writer.write_all(&frame_payload)?;
            written_size_len = size_bytes.len();
            written_payload_len = frame_payload.len();
        }

        let old_frame_len =
            command_bytes.len() + tick_bytes.len() + size_bytes.len() + frame_payload.len();
        let new_frame_len =
            command_bytes.len() + tick_bytes.len() + written_size_len + written_payload_len;
        input_offset = input_offset
            .checked_add(old_frame_len as u64)
            .ok_or_else(|| fail("input offset overflow"))?;
        output_offset = output_offset
            .checked_add(new_frame_len as u64)
            .ok_or_else(|| fail("output offset overflow"))?;
        stats.outer_frames += 1;
    }

    let new_file_info_offset =
        resolve_header_offset("FileInfo", old_file_info_offset, new_file_info_offset)?;
    let new_spawn_groups_offset = resolve_header_offset(
        "SpawnGroups",
        old_spawn_groups_offset,
        new_spawn_groups_offset,
    )?;

    writer.flush()?;
    writer.seek(SeekFrom::Start(8))?;
    writer.write_all(&new_file_info_offset.to_le_bytes())?;
    writer.write_all(&new_spawn_groups_offset.to_le_bytes())?;
    writer.flush()?;
    let output_file = writer
        .into_inner()
        .map_err(|error| fail(format!("failed to finalize output: {}", error.error())))?;
    output_file.sync_all()?;

    verify_output_structure(
        temporary,
        new_file_info_offset,
        new_spawn_groups_offset,
        stats.outer_frames,
    )?;
    Ok(stats)
}

fn resolve_header_offset(name: &str, old_offset: u32, mapped_offset: Option<u32>) -> Result<u32> {
    if old_offset == 0 {
        return Ok(0);
    }
    mapped_offset.ok_or_else(|| {
        fail(format!(
            "demo header {name} offset {old_offset} does not point to an outer frame boundary"
        ))
    })
}

fn to_u32_offset(offset: u64) -> Result<u32> {
    u32::try_from(offset).map_err(|_| fail(format!("demo offset exceeds u32: {offset}")))
}

fn read_outer_varint<R: Read>(
    reader: &mut R,
    allow_clean_eof: bool,
) -> Result<Option<(u32, Vec<u8>)>> {
    let mut bytes = Vec::with_capacity(5);
    let mut value = 0_u32;
    for index in 0..5 {
        let mut byte = [0_u8; 1];
        match reader.read_exact(&mut byte) {
            Ok(()) => {}
            Err(error)
                if allow_clean_eof
                    && index == 0
                    && error.kind() == io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(error) => return Err(fail(format!("failed to read outer varint: {error}"))),
        }
        bytes.push(byte[0]);
        if index == 4 && byte[0] & 0xf0 != 0 {
            return Err(fail("outer varint exceeds u32"));
        }
        value |= u32::from(byte[0] & 0x7f) << (index * 7);
        if byte[0] & 0x80 == 0 {
            return Ok(Some((value, bytes)));
        }
    }
    Err(fail("outer varint exceeds five bytes"))
}

fn rewrite_packet_protobuf(
    message: &[u8],
    tick: u32,
    stats: &mut RewriteStats,
) -> Result<Option<Vec<u8>>> {
    replace_unique_bytes_field(message, 3, |packet_data| {
        rewrite_netmessage_stream(packet_data, tick, stats)
    })
}

fn rewrite_full_packet_protobuf(
    message: &[u8],
    tick: u32,
    stats: &mut RewriteStats,
) -> Result<Option<Vec<u8>>> {
    replace_unique_bytes_field(message, 2, |packet| {
        rewrite_packet_protobuf(packet, tick, stats)
    })
}

fn replace_unique_bytes_field<F>(
    message: &[u8],
    target_field: u64,
    mut rewrite: F,
) -> Result<Option<Vec<u8>>>
where
    F: FnMut(&[u8]) -> Result<Option<Vec<u8>>>,
{
    let fields = parse_protobuf_fields(message)?;
    let mut matches = fields
        .iter()
        .filter(|field| field.number == target_field)
        .peekable();
    let Some(field) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(fail(format!(
            "protobuf field {target_field} occurs more than once"
        )));
    }
    if field.wire_type != 2 {
        return Err(fail(format!(
            "protobuf field {target_field} has wire type {}, expected 2",
            field.wire_type
        )));
    }
    let Some(replacement) = rewrite(&message[field.value_start..field.end])? else {
        return Ok(None);
    };

    let mut output = Vec::with_capacity(message.len() + replacement.len());
    output.extend_from_slice(&message[..field.key_end]);
    write_varint(&mut output, replacement.len() as u64);
    output.extend_from_slice(&replacement);
    output.extend_from_slice(&message[field.end..]);
    Ok(Some(output))
}

fn parse_protobuf_fields(message: &[u8]) -> Result<Vec<ProtoField>> {
    let mut fields = Vec::new();
    let mut cursor = 0;
    while cursor < message.len() {
        let (key, key_end) = read_slice_varint(message, cursor)?;
        cursor = key_end;
        let number = key >> 3;
        let wire_type = (key & 7) as u8;
        if number == 0 {
            return Err(fail("protobuf field number zero is invalid"));
        }
        let (value_start, end) = match wire_type {
            0 => {
                let value_start = cursor;
                let (_, end) = read_slice_varint(message, cursor)?;
                (value_start, end)
            }
            1 => (cursor, checked_slice_end(message, cursor, 8, "fixed64")?),
            2 => {
                let (length, value_start) = read_slice_varint(message, cursor)?;
                let length = usize::try_from(length)
                    .map_err(|_| fail("protobuf bytes length does not fit usize"))?;
                (
                    value_start,
                    checked_slice_end(message, value_start, length, "bytes")?,
                )
            }
            5 => (cursor, checked_slice_end(message, cursor, 4, "fixed32")?),
            3 | 4 => return Err(fail("protobuf group wire types are not supported")),
            _ => return Err(fail(format!("invalid protobuf wire type {wire_type}"))),
        };
        fields.push(ProtoField {
            number,
            wire_type,
            key_end,
            value_start,
            end,
        });
        cursor = end;
    }
    Ok(fields)
}

fn checked_slice_end(bytes: &[u8], start: usize, length: usize, kind: &str) -> Result<usize> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| fail(format!("protobuf {kind} length overflow")))?;
    if end > bytes.len() {
        return Err(fail(format!("protobuf {kind} extends beyond message")));
    }
    Ok(end)
}

fn read_slice_varint(bytes: &[u8], start: usize) -> Result<(u64, usize)> {
    let mut value = 0_u64;
    for index in 0..10 {
        let position = start
            .checked_add(index)
            .ok_or_else(|| fail("protobuf varint offset overflow"))?;
        let byte = *bytes
            .get(position)
            .ok_or_else(|| fail("protobuf ended inside varint"))?;
        if index == 9 && byte > 1 {
            return Err(fail("protobuf varint exceeds u64"));
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok((value, position + 1));
        }
    }
    Err(fail("protobuf varint exceeds ten bytes"))
}

fn rewrite_netmessage_stream(
    data: &[u8],
    tick: u32,
    stats: &mut RewriteStats,
) -> Result<Option<Vec<u8>>> {
    let messages = parse_netmessages(data)?;
    let targets: Vec<_> = messages
        .iter()
        .filter(|message| message.message_type == REMOVE_ALL_DECALS)
        .copied()
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }

    for target in &targets {
        let payload = read_bit_bytes(data, target.payload_start_bit, target.payload_size)?;
        validate_remove_all_decals_payload(&payload)?;
    }

    let mut writer = BitWriter::default();
    for message in &messages {
        if message.message_type != REMOVE_ALL_DECALS {
            writer.copy_bits(data, message.start_bit, message.end_bit - message.start_bit)?;
        }
    }
    let output = writer.into_bytes();
    verify_kept_messages(data, &messages, &output)?;

    let removed =
        u32::try_from(targets.len()).map_err(|_| fail("too many messages in one frame"))?;
    stats.changed_frames += 1;
    stats.removed_messages += u64::from(removed);
    stats.first_removed_tick.get_or_insert(tick);
    stats.last_removed_tick = Some(tick);
    stats.max_removed_in_frame = stats.max_removed_in_frame.max(removed);
    Ok(Some(output))
}

fn parse_netmessages(data: &[u8]) -> Result<Vec<NetMessage>> {
    let mut cursor = BitCursor::new(data);
    let mut messages = Vec::new();
    while cursor.bits_remaining() > 8 {
        let start_bit = cursor.position;
        let message_type = cursor.read_u_bit_var()?;
        let payload_size_u32 = cursor.read_varint32()?;
        let payload_size = usize::try_from(payload_size_u32)
            .map_err(|_| fail("netmessage payload size does not fit usize"))?;
        let payload_start_bit = cursor.position;
        let payload_bits = payload_size
            .checked_mul(8)
            .ok_or_else(|| fail("netmessage payload bit length overflow"))?;
        cursor.skip_bits(payload_bits)?;
        messages.push(NetMessage {
            message_type,
            payload_size,
            start_bit,
            payload_start_bit,
            end_bit: cursor.position,
        });
    }
    Ok(messages)
}

fn verify_kept_messages(input: &[u8], original: &[NetMessage], output: &[u8]) -> Result<()> {
    let rewritten = parse_netmessages(output)?;
    let kept: Vec<_> = original
        .iter()
        .filter(|message| message.message_type != REMOVE_ALL_DECALS)
        .collect();
    if rewritten.len() != kept.len() {
        return Err(fail(format!(
            "rewritten packet message count changed unexpectedly: expected {}, got {}",
            kept.len(),
            rewritten.len()
        )));
    }
    for (expected, actual) in kept.into_iter().zip(&rewritten) {
        if expected.message_type != actual.message_type
            || expected.payload_size != actual.payload_size
        {
            return Err(fail("rewritten packet changed a kept message header"));
        }
        let expected_bits = expected.end_bit - expected.start_bit;
        let actual_bits = actual.end_bit - actual.start_bit;
        if expected_bits != actual_bits
            || !bit_ranges_equal(
                input,
                expected.start_bit,
                output,
                actual.start_bit,
                expected_bits,
            )?
        {
            return Err(fail("rewritten packet changed a kept message bit range"));
        }
    }
    if rewritten
        .iter()
        .any(|message| message.message_type == REMOVE_ALL_DECALS)
    {
        return Err(fail("rewritten packet still contains message 138"));
    }
    Ok(())
}

fn validate_remove_all_decals_payload(payload: &[u8]) -> Result<()> {
    // Expected legacy schema:
    //   field 1: remove_decals = true
    //   field 2: CEntityMsg { field 1: target entity handle }
    let fields = parse_protobuf_fields(payload)?;
    if fields.len() != 2
        || fields[0].number != 1
        || fields[0].wire_type != 0
        || fields[1].number != 2
        || fields[1].wire_type != 2
    {
        return Err(fail(
            "message 138 payload does not match RemoveAllDecals schema",
        ));
    }
    let (remove_decals, end) = read_slice_varint(payload, fields[0].value_start)?;
    if end != fields[0].end || remove_decals != 1 {
        return Err(fail("message 138 does not request remove_decals=true"));
    }
    let entity = &payload[fields[1].value_start..fields[1].end];
    let entity_fields = parse_protobuf_fields(entity)?;
    if entity_fields.len() != 1 || entity_fields[0].number != 1 || entity_fields[0].wire_type != 0 {
        return Err(fail("message 138 has an unexpected entity message"));
    }
    let (target_entity, entity_end) = read_slice_varint(entity, entity_fields[0].value_start)?;
    if entity_end != entity_fields[0].end || u32::try_from(target_entity).is_err() {
        return Err(fail("message 138 entity handle is malformed"));
    }
    Ok(())
}

fn read_bit_bytes(data: &[u8], start_bit: usize, length: usize) -> Result<Vec<u8>> {
    let mut cursor = BitCursor {
        data,
        position: start_bit,
    };
    let mut output = Vec::with_capacity(length);
    for _ in 0..length {
        output.push(cursor.read_bits(8)? as u8);
    }
    Ok(output)
}

fn bit_ranges_equal(
    left: &[u8],
    left_start: usize,
    right: &[u8],
    right_start: usize,
    bit_count: usize,
) -> Result<bool> {
    for offset in 0..bit_count {
        let left_bit = bit_at(left, left_start + offset)?;
        let right_bit = bit_at(right, right_start + offset)?;
        if left_bit != right_bit {
            return Ok(false);
        }
    }
    Ok(true)
}

fn bit_at(data: &[u8], position: usize) -> Result<u8> {
    let byte = data
        .get(position / 8)
        .ok_or_else(|| fail("bit position extends beyond buffer"))?;
    Ok((byte >> (position % 8)) & 1)
}

struct BitCursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BitCursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn bits_remaining(&self) -> usize {
        self.data.len() * 8 - self.position
    }

    fn read_bits(&mut self, count: usize) -> Result<u32> {
        if count > 32 {
            return Err(fail("cannot read more than 32 bits at once"));
        }
        if count > self.bits_remaining() {
            return Err(fail("netmessage bitstream ended unexpectedly"));
        }
        let mut value = 0_u32;
        for offset in 0..count {
            value |= u32::from(bit_at(self.data, self.position + offset)?) << offset;
        }
        self.position += count;
        Ok(value)
    }

    fn skip_bits(&mut self, count: usize) -> Result<()> {
        if count > self.bits_remaining() {
            return Err(fail("netmessage payload extends beyond packet data"));
        }
        self.position += count;
        Ok(())
    }

    fn read_u_bit_var(&mut self) -> Result<u32> {
        let first = self.read_bits(6)?;
        match first & 0x30 {
            0x10 => Ok((first & 0x0f) | (self.read_bits(4)? << 4)),
            0x20 => Ok((first & 0x0f) | (self.read_bits(8)? << 4)),
            0x30 => Ok((first & 0x0f) | (self.read_bits(28)? << 4)),
            _ => Ok(first),
        }
    }

    fn read_varint32(&mut self) -> Result<u32> {
        let mut value = 0_u32;
        for index in 0..5 {
            let byte = self.read_bits(8)?;
            if index == 4 && byte & 0xf0 != 0 {
                return Err(fail("netmessage length varint exceeds u32"));
            }
            value |= (byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(fail("netmessage length varint exceeds five bytes"))
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    fn push_bit(&mut self, bit: u8) {
        if self.bit_len % 8 == 0 {
            self.bytes.push(0);
        }
        if bit != 0 {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 1 << (self.bit_len % 8);
        }
        self.bit_len += 1;
    }

    fn copy_bits(&mut self, source: &[u8], start_bit: usize, count: usize) -> Result<()> {
        for offset in 0..count {
            self.push_bit(bit_at(source, start_bit + offset)?);
        }
        Ok(())
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

fn write_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn verify_output_structure(
    output: &Path,
    expected_file_info_offset: u32,
    expected_spawn_groups_offset: u32,
    expected_frames: u64,
) -> Result<()> {
    let file = File::open(output)
        .map_err(|error| fail(format!("failed to reopen {}: {error}", output.display())))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut header = [0_u8; DEMO_HEADER_LEN];
    reader.read_exact(&mut header)?;
    if &header[..8] != DEMO_MAGIC
        || u32::from_le_bytes(header[8..12].try_into().unwrap()) != expected_file_info_offset
        || u32::from_le_bytes(header[12..16].try_into().unwrap()) != expected_spawn_groups_offset
    {
        return Err(fail("rewritten demo header verification failed"));
    }

    let mut offset = DEMO_HEADER_LEN as u64;
    let mut frames = 0_u64;
    let mut saw_file_info_offset = expected_file_info_offset == 0;
    let mut saw_spawn_groups_offset = expected_spawn_groups_offset == 0;
    loop {
        let Some((raw_command, command_bytes)) = read_outer_varint(&mut reader, true)? else {
            break;
        };
        let command = raw_command & !DEM_IS_COMPRESSED;
        if offset == u64::from(expected_file_info_offset) {
            if command != 2 {
                return Err(fail(
                    "rewritten FileInfo offset points to the wrong command",
                ));
            }
            saw_file_info_offset = true;
        }
        if offset == u64::from(expected_spawn_groups_offset) {
            if command != 15 {
                return Err(fail(
                    "rewritten SpawnGroups offset points to the wrong command",
                ));
            }
            saw_spawn_groups_offset = true;
        }
        let (_, tick_bytes) = read_outer_varint(&mut reader, false)?
            .ok_or_else(|| fail("rewritten demo ended while reading tick"))?;
        let (size, size_bytes) = read_outer_varint(&mut reader, false)?
            .ok_or_else(|| fail("rewritten demo ended while reading frame size"))?;
        let size = usize::try_from(size).map_err(|_| fail("rewritten frame size overflow"))?;
        if size > MAX_FRAME_SIZE {
            return Err(fail("rewritten demo contains an unreasonable frame size"));
        }
        let copied = io::copy(&mut reader.by_ref().take(size as u64), &mut io::sink())?;
        if copied != size as u64 {
            return Err(fail("rewritten demo ended inside an outer frame"));
        }
        offset += (command_bytes.len() + tick_bytes.len() + size_bytes.len() + size) as u64;
        frames += 1;
    }
    if frames != expected_frames {
        return Err(fail(format!(
            "rewritten demo frame count changed: expected {expected_frames}, got {frames}"
        )));
    }
    if !saw_file_info_offset || !saw_spawn_groups_offset {
        return Err(fail("rewritten demo header offset is not a frame boundary"));
    }
    Ok(())
}

fn fail(message: impl Into<String>) -> PlaybackFixError {
    PlaybackFixError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bits(writer: &mut BitWriter, value: u32, count: usize) {
        for offset in 0..count {
            writer.push_bit(((value >> offset) & 1) as u8);
        }
    }

    fn write_u_bit_var(writer: &mut BitWriter, value: u32) {
        if value < 16 {
            write_bits(writer, value, 6);
        } else if value < 256 {
            write_bits(writer, (value & 15) | 0x10, 6);
            write_bits(writer, value >> 4, 4);
        } else if value < 4096 {
            write_bits(writer, (value & 15) | 0x20, 6);
            write_bits(writer, value >> 4, 8);
        } else {
            write_bits(writer, (value & 15) | 0x30, 6);
            write_bits(writer, value >> 4, 28);
        }
    }

    fn append_message(writer: &mut BitWriter, message_type: u32, payload: &[u8]) {
        write_u_bit_var(writer, message_type);
        let mut size = Vec::new();
        write_varint(&mut size, payload.len() as u64);
        for byte in size {
            write_bits(writer, u32::from(byte), 8);
        }
        for byte in payload {
            write_bits(writer, u32::from(*byte), 8);
        }
    }

    fn remove_decals_payload(entity: u32) -> Vec<u8> {
        let mut nested = vec![0x08];
        write_varint(&mut nested, u64::from(entity));
        let mut payload = vec![0x08, 0x01, 0x12];
        write_varint(&mut payload, nested.len() as u64);
        payload.extend_from_slice(&nested);
        payload
    }

    #[test]
    fn removes_non_byte_aligned_138_and_preserves_neighbors() {
        let mut input = BitWriter::default();
        append_message(&mut input, 207, &[1, 2, 3]);
        append_message(&mut input, 138, &remove_decals_payload(2703599));
        append_message(&mut input, 76, &[4, 5, 6, 7]);
        let input = input.into_bytes();
        let mut stats = RewriteStats::default();
        let output = rewrite_netmessage_stream(&input, 4, &mut stats)
            .unwrap()
            .unwrap();
        let messages = parse_netmessages(&output).unwrap();
        assert_eq!(
            messages.iter().map(|m| m.message_type).collect::<Vec<_>>(),
            vec![207, 76]
        );
        assert_eq!(stats.removed_messages, 1);
        assert_eq!(stats.first_removed_tick, Some(4));
    }

    #[test]
    fn packet_field_replacement_preserves_unknown_fields() {
        let mut stream = BitWriter::default();
        append_message(&mut stream, 138, &remove_decals_payload(688310));
        append_message(&mut stream, 76, &[9, 8, 7]);
        let stream = stream.into_bytes();

        let mut packet = vec![0x08, 0x2a, 0x1a];
        write_varint(&mut packet, stream.len() as u64);
        packet.extend_from_slice(&stream);
        packet.extend_from_slice(&[0x25, 1, 2, 3, 4]);
        let mut stats = RewriteStats::default();
        let output = rewrite_packet_protobuf(&packet, 4234, &mut stats)
            .unwrap()
            .unwrap();
        assert!(output.starts_with(&[0x08, 0x2a, 0x1a]));
        assert!(output.ends_with(&[0x25, 1, 2, 3, 4]));
        assert_eq!(stats.removed_messages, 1);
    }

    #[test]
    fn rejects_unexpected_138_payload() {
        let mut input = BitWriter::default();
        append_message(&mut input, 138, &[0x08, 0x00]);
        let input = input.into_bytes();
        let mut stats = RewriteStats::default();
        let error = rewrite_netmessage_stream(&input, 1, &mut stats).unwrap_err();
        assert!(error.to_string().contains("RemoveAllDecals"));
    }

    #[test]
    fn no_target_is_a_noop() {
        let mut input = BitWriter::default();
        append_message(&mut input, 207, &[1, 2, 3]);
        let input = input.into_bytes();
        let mut stats = RewriteStats::default();
        assert!(rewrite_netmessage_stream(&input, 1, &mut stats)
            .unwrap()
            .is_none());
    }

    #[test]
    fn rejects_u32_varint_overflow() {
        let outer = [0xff, 0xff, 0xff, 0xff, 0x10];
        assert!(read_outer_varint(&mut outer.as_slice(), false).is_err());

        let mut inner = BitWriter::default();
        write_u_bit_var(&mut inner, 76);
        for byte in outer {
            write_bits(&mut inner, u32::from(byte), 8);
        }
        assert!(parse_netmessages(&inner.into_bytes()).is_err());
    }
}
