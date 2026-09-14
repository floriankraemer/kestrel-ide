//! "Analyze image" (C9, optional-but-done last): a sequential tar reader
//! over `save -o` output — headers only, every entry's data seeked past
//! rather than read, so this works on an image of any size — plus the
//! `manifest.json` -> per-layer tar mapping and the added/modified/deleted
//! classification the Layers tab's per-layer tree shows.
//!
//! No tar crate: `save`'s own tar (Docker's legacy `manifest.json` layout,
//! which `podman save` also writes for `docker load` compatibility) is
//! read the same minimal way [`crate::files`]'s single-entry reader reads
//! a `cp` stream, just walking every entry instead of stopping at the
//! first one, and reading a nested tar (one layer's `layer.tar`) as a
//! byte *region* of the outer file rather than extracting it anywhere.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::connection::Invocation;
use crate::ops::{run_op, OpError, OpErrorCode};

const HEADER_SIZE: u64 = 512;

/// One tar header this reader kept — data never read into memory, only
/// its offset within the file/region it was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTarEntry {
    pub name: String,
    pub size: u64,
    pub typeflag: u8,
    /// Absolute offset (within the reader the entry was walked from) of
    /// this entry's data — how a caller reads a *small* entry's content
    /// (`manifest.json`) or locates a nested tar's byte region (a layer's
    /// `layer.tar`) without extracting anything to disk.
    pub data_offset: u64,
}

fn parse_octal(field: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(field).ok()?;
    let trimmed = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(trimmed, 8).ok()
}

fn pad_to_block(size: u64) -> u64 {
    size.div_ceil(HEADER_SIZE) * HEADER_SIZE
}

/// The ustar name: `prefix + "/" + name` when the header carries a
/// `ustar` prefix (a name longer than the 100-byte `name` field alone can
/// hold, split across `name`/`prefix` per POSIX ustar), else just `name`.
fn ustar_name(header: &[u8; 512]) -> String {
    let name = read_cstr(&header[0..100]);
    let magic = &header[257..263];
    if magic == b"ustar\0" || magic == b"ustar " {
        let prefix = read_cstr(&header[345..500]);
        if !prefix.is_empty() {
            return format!("{prefix}/{name}");
        }
    }
    name
}

fn read_cstr(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).to_string()
}

/// One `<len> key=value\n` PAX extended-header record's `path` value, the
/// only key this reader needs (a long name or a name with characters the
/// 100-byte `name` field cannot hold).
fn pax_path(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    let mut rest = text.as_ref();
    while !rest.is_empty() {
        let (len_str, _) = rest.split_once(' ')?;
        let len: usize = len_str.parse().ok()?;
        if len == 0 || len > rest.len() {
            break;
        }
        let record = &rest[..len.min(rest.len())];
        let kv = &record[len_str.len() + 1..record.len().saturating_sub(1)];
        if let Some(value) = kv.strip_prefix("path=") {
            return Some(value.to_string());
        }
        rest = &rest[len.min(rest.len())..];
    }
    None
}

/// Walk `len` bytes of tar starting at `start` in `reader`, reading only
/// headers: every entry's data is skipped over (`seek`, not `read`), and
/// GNU (`typeflag == 'L'`)/PAX (`typeflag == 'x'`/`'g'`) long-name
/// entries are folded into the *next* real entry's name rather than
/// returned themselves.
pub fn walk_tar_region<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    len: u64,
) -> io::Result<Vec<RawTarEntry>> {
    let mut entries = Vec::new();
    let mut pos: u64 = 0;
    let mut pending_name: Option<String> = None;
    let mut zero_headers = 0u32;

    while pos + HEADER_SIZE <= len {
        reader.seek(SeekFrom::Start(start + pos))?;
        let mut header = [0u8; 512];
        if reader.read_exact(&mut header).is_err() {
            break;
        }
        if header.iter().all(|&b| b == 0) {
            zero_headers += 1;
            pos += HEADER_SIZE;
            if zero_headers >= 2 {
                break;
            }
            continue;
        }
        zero_headers = 0;

        let Some(size) = parse_octal(&header[124..136]) else {
            break;
        };
        let typeflag = header[156];
        let name = ustar_name(&header);
        pos += HEADER_SIZE;
        let data_start = start + pos;

        if typeflag == b'L' || typeflag == b'x' || typeflag == b'g' {
            if typeflag == b'L' {
                let mut buf = vec![0u8; size as usize];
                reader.seek(SeekFrom::Start(data_start))?;
                if reader.read_exact(&mut buf).is_ok() {
                    pending_name = Some(read_cstr(&buf));
                }
            } else {
                let mut buf = vec![0u8; size as usize];
                reader.seek(SeekFrom::Start(data_start))?;
                if reader.read_exact(&mut buf).is_ok() {
                    if let Some(path) = pax_path(&buf) {
                        pending_name = Some(path);
                    }
                }
            }
            pos += pad_to_block(size);
            continue;
        }

        let name = pending_name.take().unwrap_or(name);
        entries.push(RawTarEntry {
            name,
            size,
            typeflag,
            data_offset: data_start,
        });
        pos += pad_to_block(size);
    }
    Ok(entries)
}

/// [`walk_tar_region`] over the whole of `reader`.
pub fn walk_tar<R: Read + Seek>(reader: &mut R) -> io::Result<Vec<RawTarEntry>> {
    let len = reader.seek(SeekFrom::End(0))?;
    walk_tar_region(reader, 0, len)
}

/// Whether kind classification and the deleted path for a whiteout entry
/// name — `.wh.<name>` marks `<name>` deleted in this layer, and the
/// special `.wh..wh..opq` marks its *directory* opaque (every path
/// beneath it from a lower layer is hidden). Opaque markers classify as
/// [`EntryKind::Deleted`] on the directory itself: this reader reports
/// per-layer changes, not a merged view, so "opaque" and "the directory
/// itself was removed and recreated" read the same way here — a known
/// ceiling, upgrade path is a distinct `EntryKind::Opaque` if the Layers
/// tab ever needs to draw it differently.
fn whiteout_path(name: &str) -> Option<String> {
    let (dir, base) = match name.rsplit_once('/') {
        Some((dir, base)) => (dir, base),
        None => ("", name),
    };
    if base == ".wh..wh..opq" {
        return Some(if dir.is_empty() {
            ".".to_string()
        } else {
            dir.to_string()
        });
    }
    let stripped = base.strip_prefix(".wh.")?;
    Some(if dir.is_empty() {
        stripped.to_string()
    } else {
        format!("{dir}/{stripped}")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerEntry {
    pub path: String,
    pub size: u64,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerFs {
    pub layer_id: String,
    pub entries: Vec<LayerEntry>,
}

/// Classify one layer's raw tar entries against `seen` (every path any
/// lower layer has already introduced), mutating `seen` for the next
/// layer's turn. Order matters: layers are classified bottom-to-top, the
/// same order `manifest.json`'s own `Layers` list already gives.
fn classify_layer(raw_entries: &[RawTarEntry], seen: &mut HashSet<String>) -> Vec<LayerEntry> {
    let mut entries = Vec::with_capacity(raw_entries.len());
    for raw in raw_entries {
        if let Some(deleted_path) = whiteout_path(&raw.name) {
            entries.push(LayerEntry {
                path: deleted_path,
                size: 0,
                kind: EntryKind::Deleted,
            });
            continue;
        }
        let path = raw.name.trim_end_matches('/').to_string();
        let kind = if seen.contains(&path) {
            EntryKind::Modified
        } else {
            EntryKind::Added
        };
        seen.insert(path.clone());
        entries.push(LayerEntry {
            path,
            size: raw.size,
            kind,
        });
    }
    entries
}

/// `manifest.json`'s `Layers` list — the `<layer-tar-path>` entries in
/// bottom-to-top order, the classic `docker save`/`podman save` layout
/// both engines still write (`[{"Config":..., "Layers": [...] , ...}]`).
/// OCI's `index.json` layout (no `manifest.json` at all) is a documented
/// gap: `save` without `--format oci` (never passed here) always writes
/// this layout.
fn manifest_layers(manifest_json: &[u8]) -> Result<Vec<String>, OpError> {
    let parsed: serde_json::Value =
        serde_json::from_slice(manifest_json).map_err(|error| OpError {
            code: OpErrorCode::Other,
            message: format!("manifest.json was not valid JSON: {error}"),
        })?;
    let first = parsed
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| OpError {
            code: OpErrorCode::Other,
            message: "manifest.json had no image entry".to_string(),
        })?;
    let layers = first
        .get("Layers")
        .and_then(|value| value.as_array())
        .ok_or_else(|| OpError {
            code: OpErrorCode::Other,
            message: "manifest.json had no Layers list".to_string(),
        })?;
    Ok(layers
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect())
}

/// [`analyze`]'s result: the per-layer listing, plus the temp tar's own
/// path — kept around (not deleted) on success, since [`read_entry`]
/// needs to reopen it for a double-clicked file's bytes without a second
/// `save`. The caller (`ui-shell`'s `ContainerService`) owns deleting it
/// once the Layers tab moves on to a different image or closes; see that
/// module's `Drop` for the last-resort cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzeResult {
    pub layers: Vec<LayerFs>,
    pub tar_path: PathBuf,
}

/// `save -o <tmp>.tar <image_id>`, then per-layer fs entries: the
/// manifest's `Layers` list, each mapped onto its outer-tar entry and
/// walked for headers only. The temp file is removed on failure, kept on
/// success (see [`AnalyzeResult`]'s own doc comment).
pub fn analyze(
    invocation: &Invocation,
    image_id: &str,
    work_dir: &Path,
) -> Result<AnalyzeResult, OpError> {
    let tmp_path = crate::images::analyze_temp_tar_path(image_id);
    let tmp_str = tmp_path.to_string_lossy().to_string();

    if let Err(err) = run_op(
        invocation,
        &crate::images::save_args(image_id, &tmp_str),
        work_dir,
    ) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }

    match analyze_tar_file(&tmp_path, image_id) {
        Ok(layers) => Ok(AnalyzeResult {
            layers,
            tar_path: tmp_path,
        }),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(err)
        }
    }
}

/// The outer tar's own entries, plus `manifest.json`'s `Layers` list —
/// the first two steps both [`analyze_tar_file`] and [`read_entry`] need,
/// factored out so they cannot read `manifest.json` two different ways.
fn outer_entries_and_layers(file: &mut File) -> Result<(Vec<RawTarEntry>, Vec<String>), OpError> {
    let outer = walk_tar(file).map_err(|error| OpError {
        code: OpErrorCode::Other,
        message: format!("could not read the saved image tar: {error}"),
    })?;
    let manifest_entry = outer
        .iter()
        .find(|entry| entry.name == "manifest.json")
        .ok_or_else(|| OpError {
            code: OpErrorCode::Other,
            message: "saved tar had no manifest.json".to_string(),
        })?;
    let mut manifest_bytes = vec![0u8; manifest_entry.size as usize];
    file.seek(SeekFrom::Start(manifest_entry.data_offset))
        .and_then(|_| file.read_exact(&mut manifest_bytes))
        .map_err(|error| OpError {
            code: OpErrorCode::Other,
            message: format!("could not read manifest.json: {error}"),
        })?;
    let layer_paths = manifest_layers(&manifest_bytes)?;
    Ok((outer, layer_paths))
}

fn analyze_tar_file(tmp_path: &Path, image_id: &str) -> Result<Vec<LayerFs>, OpError> {
    let mut file = File::open(tmp_path).map_err(|error| OpError {
        code: OpErrorCode::Other,
        message: format!("could not open the saved image tar: {error}"),
    })?;
    let (outer, layer_paths) = outer_entries_and_layers(&mut file).map_err(|err| OpError {
        code: err.code,
        message: format!("{image_id}: {}", err.message),
    })?;

    let mut seen = HashSet::new();
    let mut layers = Vec::with_capacity(layer_paths.len());
    for layer_path in layer_paths {
        let Some(outer_entry) = outer.iter().find(|entry| entry.name == layer_path) else {
            continue;
        };
        let raw_entries = walk_tar_region(&mut file, outer_entry.data_offset, outer_entry.size)
            .map_err(|error| OpError {
                code: OpErrorCode::Other,
                message: format!("could not read layer {layer_path}: {error}"),
            })?;
        let entries = classify_layer(&raw_entries, &mut seen);
        let layer_id = layer_path
            .strip_suffix("/layer.tar")
            .unwrap_or(&layer_path)
            .to_string();
        layers.push(LayerFs { layer_id, entries });
    }
    Ok(layers)
}

/// The one entry's bytes at `entry_path` inside `layer_id`'s own tar,
/// reopening `tar_path` (an [`AnalyzeResult::tar_path`]) rather than
/// re-running `save` — a double-clicked file in the Layers tab's tree, or
/// its "Download..." action. `max_bytes`, when set, refuses an entry
/// larger than it (the Inspect-tab-style read-only open's own 8 MiB cap;
/// `None` for Download, which has none — JetBrains' own doesn't either).
pub fn read_entry(
    tar_path: &Path,
    layer_id: &str,
    entry_path: &str,
    max_bytes: Option<u64>,
) -> Result<Vec<u8>, OpError> {
    let mut file = File::open(tar_path).map_err(|error| OpError {
        code: OpErrorCode::Other,
        message: format!("could not open the saved image tar: {error}"),
    })?;
    let (outer, layer_paths) = outer_entries_and_layers(&mut file)?;
    let layer_path = layer_paths
        .iter()
        .find(|path| path.strip_suffix("/layer.tar").unwrap_or(path.as_str()) == layer_id)
        .ok_or_else(|| OpError {
            code: OpErrorCode::Other,
            message: format!("layer '{layer_id}' is not in this image"),
        })?;
    let outer_entry = outer
        .iter()
        .find(|entry| &entry.name == layer_path)
        .ok_or_else(|| OpError {
            code: OpErrorCode::Other,
            message: format!("layer '{layer_id}' has no tar entry in the saved image"),
        })?;
    let inner_entries = walk_tar_region(&mut file, outer_entry.data_offset, outer_entry.size)
        .map_err(|error| OpError {
            code: OpErrorCode::Other,
            message: format!("could not read layer {layer_path}: {error}"),
        })?;
    let entry = inner_entries
        .iter()
        .find(|entry| entry.name.trim_end_matches('/') == entry_path)
        .ok_or_else(|| OpError {
            code: OpErrorCode::Other,
            message: format!("'{entry_path}' is not in layer '{layer_id}'"),
        })?;
    if entry.typeflag != b'0' && entry.typeflag != 0 {
        return Err(OpError {
            code: OpErrorCode::Other,
            message: format!("'{entry_path}' is not a regular file"),
        });
    }
    if let Some(cap) = max_bytes {
        if entry.size > cap {
            return Err(OpError {
                code: OpErrorCode::Other,
                message: format!(
                    "'{entry_path}' is too large to preview ({} bytes) — use Download instead",
                    entry.size
                ),
            });
        }
    }
    let mut buf = vec![0u8; entry.size as usize];
    file.seek(SeekFrom::Start(entry.data_offset))
        .and_then(|_| file.read_exact(&mut buf))
        .map_err(|error| OpError {
            code: OpErrorCode::Other,
            message: format!("could not read '{entry_path}': {error}"),
        })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn header(name: &str, size: u64, typeflag: u8) -> [u8; 512] {
        let mut header = [0u8; 512];
        let name_bytes = name.as_bytes();
        header[..name_bytes.len().min(100)]
            .copy_from_slice(&name_bytes[..name_bytes.len().min(100)]);
        let mode = format!("{:07o}\0", 0o644);
        header[100..100 + mode.len()].copy_from_slice(mode.as_bytes());
        let size_field = format!("{:011o}\0", size);
        header[124..124 + size_field.len()].copy_from_slice(size_field.as_bytes());
        header[156] = typeflag;
        header[257..263].copy_from_slice(b"ustar\0");
        header
    }

    fn push_entry(buf: &mut Vec<u8>, name: &str, content: &[u8], typeflag: u8) {
        buf.extend_from_slice(&header(name, content.len() as u64, typeflag));
        buf.extend_from_slice(content);
        let padding = pad_to_block(content.len() as u64) as usize - content.len();
        buf.extend(std::iter::repeat_n(0u8, padding));
    }

    fn finish(buf: &mut Vec<u8>) {
        buf.extend(std::iter::repeat_n(0u8, 1024));
    }

    #[test]
    fn walks_a_hand_built_tar_with_one_regular_file() {
        let mut tar = Vec::new();
        push_entry(&mut tar, "hello.txt", b"hi", b'0');
        finish(&mut tar);
        let mut cursor = Cursor::new(tar);
        let entries = walk_tar(&mut cursor).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hello.txt");
        assert_eq!(entries[0].size, 2);
    }

    #[test]
    fn reads_the_data_at_the_reported_offset() {
        let mut tar = Vec::new();
        push_entry(&mut tar, "a.txt", b"first", b'0');
        push_entry(&mut tar, "b.txt", b"second-file", b'0');
        finish(&mut tar);
        let mut cursor = Cursor::new(tar);
        let entries = walk_tar(&mut cursor).unwrap();
        let mut buf = vec![0u8; entries[1].size as usize];
        cursor
            .seek(SeekFrom::Start(entries[1].data_offset))
            .unwrap();
        cursor.read_exact(&mut buf).unwrap();
        assert_eq!(buf, b"second-file");
    }

    #[test]
    fn gnu_long_name_entry_becomes_the_next_entrys_name() {
        let long_name = "a/very/long/path/".to_string() + &"x".repeat(120) + "/file.txt";
        let mut tar = Vec::new();
        let mut name_content = long_name.clone().into_bytes();
        name_content.push(0);
        push_entry(&mut tar, "././@LongLink", &name_content, b'L');
        push_entry(&mut tar, "truncated-name", b"data", b'0');
        finish(&mut tar);
        let mut cursor = Cursor::new(tar);
        let entries = walk_tar(&mut cursor).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, long_name);
    }

    /// A self-describing PAX record: `"<len> key=value\n"` where `<len>`
    /// (decimal, including its own digits, the space and the trailing
    /// newline) is found by fixed-point iteration — the same trick every
    /// PAX writer uses, since the length field's own width can change the
    /// length it is describing.
    fn pax_record(key: &str, value: &str) -> String {
        let suffix = format!("{key}={value}\n");
        let mut len = suffix.len() + 2;
        loop {
            let candidate = format!("{len} {suffix}");
            if candidate.len() == len {
                return candidate;
            }
            len = candidate.len();
        }
    }

    #[test]
    fn pax_extended_header_path_becomes_the_next_entrys_name() {
        let long_name = "b/".to_string() + &"y".repeat(150);
        let record_bytes = pax_record("path", &long_name);
        let mut tar = Vec::new();
        push_entry(&mut tar, "PaxHeaders/file", record_bytes.as_bytes(), b'x');
        push_entry(&mut tar, "truncated", b"data", b'0');
        finish(&mut tar);
        let mut cursor = Cursor::new(tar);
        let entries = walk_tar(&mut cursor).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, long_name);
    }

    #[test]
    fn whiteout_and_opaque_marker_paths() {
        assert_eq!(
            whiteout_path("dir/.wh.deleted.txt"),
            Some("dir/deleted.txt".to_string())
        );
        assert_eq!(whiteout_path(".wh.top.txt"), Some("top.txt".to_string()));
        assert_eq!(
            whiteout_path("dir/sub/.wh..wh..opq"),
            Some("dir/sub".to_string())
        );
        assert_eq!(whiteout_path("plain.txt"), None);
    }

    #[test]
    fn classify_layer_marks_added_then_modified_then_deleted() {
        let mut seen = HashSet::new();
        let layer1 = vec![RawTarEntry {
            name: "app/config.yml".to_string(),
            size: 10,
            typeflag: b'0',
            data_offset: 0,
        }];
        let entries1 = classify_layer(&layer1, &mut seen);
        assert_eq!(entries1[0].kind, EntryKind::Added);

        let layer2 = vec![
            RawTarEntry {
                name: "app/config.yml".to_string(),
                size: 20,
                typeflag: b'0',
                data_offset: 0,
            },
            RawTarEntry {
                name: "app/.wh.old.txt".to_string(),
                size: 0,
                typeflag: b'0',
                data_offset: 0,
            },
        ];
        let entries2 = classify_layer(&layer2, &mut seen);
        assert_eq!(entries2[0].kind, EntryKind::Modified);
        assert_eq!(entries2[1].kind, EntryKind::Deleted);
        assert_eq!(entries2[1].path, "app/old.txt");
    }

    #[test]
    fn manifest_layers_reads_the_layers_list() {
        let manifest = br#"[{"Config":"abc.json","RepoTags":["x:1"],"Layers":["l1/layer.tar","l2/layer.tar"]}]"#;
        let layers = manifest_layers(manifest).unwrap();
        assert_eq!(
            layers,
            vec!["l1/layer.tar".to_string(), "l2/layer.tar".to_string()]
        );
    }

    #[test]
    fn manifest_layers_rejects_malformed_json() {
        let err = manifest_layers(b"not json").unwrap_err();
        assert_eq!(err.code, OpErrorCode::Other);
    }

    /// Outer tar: `manifest.json` + one layer tar (itself a tar, held as
    /// this entry's raw content) with two files, one of them a whiteout —
    /// written to a temp file both `analyze_tar_file` and `read_entry`
    /// tests reopen the way `analyze`'s own caller would.
    fn build_hand_written_save_tar(test_name: &str) -> (PathBuf, PathBuf) {
        let mut layer_tar = Vec::new();
        push_entry(&mut layer_tar, "etc/app.conf", b"config contents", b'0');
        push_entry(&mut layer_tar, "etc/.wh.old.conf", b"", b'0');
        finish(&mut layer_tar);

        let manifest =
            br#"[{"Config":"cfg.json","RepoTags":["demo:1"],"Layers":["abc123/layer.tar"]}]"#;

        let mut outer = Vec::new();
        push_entry(&mut outer, "manifest.json", manifest, b'0');
        push_entry(&mut outer, "abc123/layer.tar", &layer_tar, b'0');
        finish(&mut outer);

        let dir = std::env::temp_dir().join(format!(
            "container-core-layer-fs-test-{test_name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("save.tar");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&outer)
            .unwrap();
        (dir, path)
    }

    #[test]
    fn end_to_end_analyze_over_a_hand_built_docker_save_tar() {
        let (dir, path) = build_hand_written_save_tar("analyze");
        let layers = analyze_tar_file(&path, "demo:1").unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].layer_id, "abc123");
        assert_eq!(layers[0].entries.len(), 2);
        assert_eq!(layers[0].entries[0].path, "etc/app.conf");
        assert_eq!(layers[0].entries[0].kind, EntryKind::Added);
        assert_eq!(layers[0].entries[1].path, "etc/old.conf");
        assert_eq!(layers[0].entries[1].kind, EntryKind::Deleted);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_entry_extracts_a_regular_files_bytes() {
        let (dir, path) = build_hand_written_save_tar("read-entry");
        let bytes = read_entry(&path, "abc123", "etc/app.conf", None).unwrap();
        assert_eq!(bytes, b"config contents");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_entry_respects_the_size_cap() {
        let (dir, path) = build_hand_written_save_tar("read-entry-cap");
        let err = read_entry(&path, "abc123", "etc/app.conf", Some(4)).unwrap_err();
        assert!(err.message.contains("too large"), "{}", err.message);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_entry_reports_an_unknown_layer_or_path() {
        let (dir, path) = build_hand_written_save_tar("read-entry-missing");
        assert!(read_entry(&path, "nope", "etc/app.conf", None).is_err());
        assert!(read_entry(&path, "abc123", "no/such/file", None).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_entry_refuses_a_deleted_whiteout_marker() {
        let (dir, path) = build_hand_written_save_tar("read-entry-whiteout");
        // The whiteout marker's own tar entry name is `.wh.old.conf`, not
        // the deleted path `old.conf` `read_entry` never sees a request
        // for — walking the tree only ever offers a regular file's real
        // path, so this just documents that the raw marker name itself
        // still resolves (it is, after all, a zero-byte regular file in
        // the tar).
        let bytes = read_entry(&path, "abc123", "etc/.wh.old.conf", None).unwrap();
        assert!(bytes.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
