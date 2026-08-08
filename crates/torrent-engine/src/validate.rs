//! Defensive structural validation of v1 torrent metainfo.
//!
//! Metainfo coming from outside (peers, files, the network) is untrusted:
//! before it reaches `Session::add_torrent` it is checked against
//! [`MetainfoLimits`] and against path-safety rules so that a malicious
//! torrent cannot make the engine write outside the output folder or exhaust
//! resources. Locally created torrents go through the same structural checks
//! (without the size limits, which the creator controls).

use librqbit::TorrentMetaV1Info;

use crate::types::MetainfoLimits;
use crate::{Error, Result};

/// Render a component list as a lossy `/`-joined string for error messages.
fn display_path(components: &[&[u8]]) -> String {
    components
        .iter()
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// True for Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`,
/// `COM1`-`COM9`, `LPT1`-`LPT9`), case-insensitively and including any
/// extension (`CON.txt` is reserved too).
fn is_windows_reserved_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    for reserved in ["CON", "PRN", "AUX", "NUL"] {
        if stem.eq_ignore_ascii_case(reserved) {
            return true;
        }
    }
    let is_serial = |prefix: &str| {
        stem.len() == 4
            && stem[..3].eq_ignore_ascii_case(prefix)
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0'
    };
    is_serial("COM") || is_serial("LPT")
}

/// Validate one path component (or the torrent name, which becomes the
/// output subfolder) for portability and safety.
fn validate_component(component: &[u8], limits: Option<&MetainfoLimits>, path: &str) -> Result<()> {
    let invalid = |reason: &'static str| Error::InvalidPathComponent {
        path: path.to_owned(),
        reason,
    };
    if component.is_empty() {
        return Err(invalid("empty path component"));
    }
    let text =
        std::str::from_utf8(component).map_err(|_| invalid("component is not valid UTF-8"))?;
    if text == "." || text == ".." {
        return Err(invalid("dot components are not allowed"));
    }
    if text.contains(['/', '\\']) {
        return Err(invalid("component contains a path separator"));
    }
    if text.chars().any(|c| c.is_ascii_control()) {
        return Err(invalid("component contains ASCII control characters"));
    }
    if text.ends_with('.') || text.ends_with(' ') {
        return Err(invalid("component ends with a dot or space"));
    }
    if text.len() == 2 && text.as_bytes()[0].is_ascii_alphabetic() && text.as_bytes()[1] == b':' {
        return Err(invalid("component resembles a Windows drive prefix"));
    }
    if is_windows_reserved_name(text) {
        return Err(invalid("component is a reserved Windows device name"));
    }
    if let Some(limits) = limits
        && component.len() > limits.max_component_bytes
    {
        return Err(Error::LimitExceeded {
            limit: "path component bytes",
            value: component.len() as u64,
            max: limits.max_component_bytes as u64,
        });
    }
    Ok(())
}

/// Raw view of one file entry in the metainfo.
struct FileEntry<'a> {
    components: Vec<&'a [u8]>,
    length: u64,
    has_symlink: bool,
}

/// Extract file entries in raw component form from parsed v1 metainfo.
fn file_entries<B: AsRef<[u8]>>(info: &TorrentMetaV1Info<B>) -> Result<Vec<FileEntry<'_>>> {
    if let Some(files) = &info.files {
        if files.is_empty() {
            return Err(Error::EmptyContent);
        }
        return Ok(files
            .iter()
            .map(|f| FileEntry {
                components: f.path.iter().map(AsRef::as_ref).collect(),
                length: f.length,
                has_symlink: f.symlink_path.is_some(),
            })
            .collect());
    }
    let length = info
        .length
        .ok_or_else(|| Error::InvalidMetainfo("single-file torrent without a length".to_owned()))?;
    let name = info
        .name
        .as_ref()
        .ok_or_else(|| Error::InvalidMetainfo("torrent without a name".to_owned()))?;
    Ok(vec![FileEntry {
        components: vec![name.as_ref()],
        length,
        has_symlink: info.symlink_path.is_some(),
    }])
}

/// Validate parsed v1 metainfo.
///
/// Structural rules are always enforced: a valid portable torrent name;
/// non-empty, valid-UTF-8, portable components; no symlink entries; no
/// duplicate or prefix-colliding file paths; non-zero total content. When
/// `limits` is provided, the numeric [`MetainfoLimits`] are enforced as well.
pub(crate) fn validate_metainfo<B: AsRef<[u8]>>(
    info: &TorrentMetaV1Info<B>,
    limits: Option<&MetainfoLimits>,
) -> Result<()> {
    // The torrent name is required and becomes the output subfolder on disk.
    let name = info
        .name
        .as_ref()
        .ok_or_else(|| Error::InvalidMetainfo("torrent without a name".to_owned()))?;
    validate_component(name.as_ref(), limits, "<torrent name>")?;

    let entries = file_entries(info)?;
    if let Some(limits) = limits
        && entries.len() > limits.max_files
    {
        return Err(Error::LimitExceeded {
            limit: "file count",
            value: entries.len() as u64,
            max: limits.max_files as u64,
        });
    }

    let mut total_bytes: u64 = 0;
    for entry in &entries {
        let path = display_path(&entry.components);
        if entry.has_symlink {
            return Err(Error::InvalidPathComponent {
                path,
                reason: "symlink entries are not supported",
            });
        }
        if entry.components.is_empty() {
            return Err(Error::InvalidPathComponent {
                path,
                reason: "file has an empty path",
            });
        }
        if let Some(limits) = limits {
            if entry.components.len() > limits.max_path_components {
                return Err(Error::LimitExceeded {
                    limit: "path components per file",
                    value: entry.components.len() as u64,
                    max: limits.max_path_components as u64,
                });
            }
            // Components plus one separator byte per join.
            let path_bytes = entry.components.iter().map(|c| c.len()).sum::<usize>()
                + entry.components.len()
                - 1;
            if path_bytes > limits.max_path_bytes {
                return Err(Error::LimitExceeded {
                    limit: "relative path bytes",
                    value: path_bytes as u64,
                    max: limits.max_path_bytes as u64,
                });
            }
        }
        for component in &entry.components {
            validate_component(component, limits, &path)?;
        }
        total_bytes = total_bytes.checked_add(entry.length).ok_or_else(|| {
            Error::InvalidMetainfo("declared file lengths overflow u64".to_owned())
        })?;
    }

    // Exact duplicates and file-vs-directory prefix collisions. After sorting,
    // colliding paths are adjacent.
    let mut paths: Vec<&Vec<&[u8]>> = entries.iter().map(|e| &e.components).collect();
    paths.sort_unstable();
    for pair in paths.windows(2) {
        let (ancestor, descendant) = (pair[0], pair[1]);
        if ancestor == descendant {
            return Err(Error::DuplicateFilePath(display_path(ancestor)));
        }
        if descendant.starts_with(ancestor) {
            return Err(Error::PrefixPathCollision {
                directory: display_path(ancestor),
                file: display_path(descendant),
            });
        }
    }

    if total_bytes == 0 {
        return Err(Error::EmptyContent);
    }
    if let Some(limits) = limits
        && total_bytes > limits.max_total_bytes
    {
        return Err(Error::LimitExceeded {
            limit: "total content bytes",
            value: total_bytes,
            max: limits.max_total_bytes,
        });
    }
    Ok(())
}

/// Number of files declared by already-validated metainfo.
pub(crate) fn file_count<B: AsRef<[u8]>>(info: &TorrentMetaV1Info<B>) -> usize {
    info.files.as_ref().map_or(1, Vec::len)
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Minimal bencode writer for crafting v1 metainfo in tests.

    pub fn bstr(s: &[u8]) -> Vec<u8> {
        let mut v = s.len().to_string().into_bytes();
        v.push(b':');
        v.extend_from_slice(s);
        v
    }

    pub fn bint(i: u64) -> Vec<u8> {
        format!("i{i}e").into_bytes()
    }

    pub fn blist(items: Vec<Vec<u8>>) -> Vec<u8> {
        let mut v = vec![b'l'];
        for item in items {
            v.extend_from_slice(&item);
        }
        v.push(b'e');
        v
    }

    /// `pairs` must be sorted by key, as bencode requires.
    pub fn bdict(pairs: Vec<(&[u8], Vec<u8>)>) -> Vec<u8> {
        let mut v = vec![b'd'];
        for (key, value) in pairs {
            v.extend_from_slice(&bstr(key));
            v.extend_from_slice(&value);
        }
        v.push(b'e');
        v
    }

    /// A complete `.torrent` payload wrapping the given info dict.
    pub fn torrent_bytes(info_dict: Vec<u8>) -> Vec<u8> {
        bdict(vec![(b"info", info_dict)])
    }

    /// Info dict for a multi-file torrent. `files` is (path components, len).
    pub fn multifile_info(name: &[u8], files: Vec<(Vec<&[u8]>, u64)>) -> Vec<u8> {
        let file_dicts = files
            .into_iter()
            .map(|(path, length)| {
                bdict(vec![
                    (b"length", bint(length)),
                    (b"path", blist(path.into_iter().map(bstr).collect())),
                ])
            })
            .collect();
        bdict(vec![
            (b"files", blist(file_dicts)),
            (b"name", bstr(name)),
            (b"piece length", bint(16_384)),
            (b"pieces", bstr(&[0u8; 20])),
        ])
    }

    /// Info dict for a single-file torrent.
    pub fn singlefile_info(name: &[u8], length: u64) -> Vec<u8> {
        bdict(vec![
            (b"length", bint(length)),
            (b"name", bstr(name)),
            (b"piece length", bint(16_384)),
            (b"pieces", bstr(&[0u8; 20])),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use librqbit::ByteBuf;

    fn parse(bytes: &[u8]) -> librqbit::TorrentMetaV1<ByteBuf<'_>> {
        librqbit::torrent_from_bytes::<ByteBuf>(bytes).expect("test metainfo must parse")
    }

    fn validate(bytes: &[u8], limits: Option<&MetainfoLimits>) -> Result<()> {
        let parsed = parse(bytes);
        validate_metainfo(&parsed.info, limits)
    }

    fn multifile(name: &[u8], files: Vec<(Vec<&[u8]>, u64)>) -> Vec<u8> {
        torrent_bytes(multifile_info(name, files))
    }

    #[test]
    fn valid_metainfo_passes_default_limits() {
        let bytes = multifile(
            b"release",
            vec![
                (vec![b"a.bin"], 1000),
                (vec![b"sub", b"b.bin"], 2000),
                (vec!["ünïcödé 名前".as_bytes()], 1),
            ],
        );
        validate(&bytes, Some(&MetainfoLimits::default())).unwrap();

        let single = torrent_bytes(singlefile_info(b"file.bin", 42));
        validate(&single, Some(&MetainfoLimits::default())).unwrap();
    }

    #[test]
    fn rejects_missing_name_and_length() {
        // Single-file torrent without a name key.
        let no_name = torrent_bytes(bdict(vec![
            (b"length", bint(10)),
            (b"piece length", bint(16_384)),
            (b"pieces", bstr(&[0u8; 20])),
        ]));
        assert!(matches!(
            validate(&no_name, None),
            Err(Error::InvalidMetainfo(_))
        ));

        // Multi-file torrent without a name key.
        let no_name_multi = torrent_bytes(bdict(vec![
            (
                b"files",
                blist(vec![bdict(vec![
                    (b"length", bint(10)),
                    (b"path", blist(vec![bstr(b"f")])),
                ])]),
            ),
            (b"piece length", bint(16_384)),
            (b"pieces", bstr(&[0u8; 20])),
        ]));
        assert!(matches!(
            validate(&no_name_multi, None),
            Err(Error::InvalidMetainfo(_))
        ));

        // Single-file torrent without a length key.
        let no_length = torrent_bytes(bdict(vec![
            (b"name", bstr(b"x")),
            (b"piece length", bint(16_384)),
            (b"pieces", bstr(&[0u8; 20])),
        ]));
        assert!(matches!(
            validate(&no_length, None),
            Err(Error::InvalidMetainfo(_))
        ));

        // Empty file list means no content.
        let empty_files = multifile(b"release", vec![]);
        assert!(matches!(
            validate(&empty_files, None),
            Err(Error::EmptyContent)
        ));
    }

    #[test]
    fn rejects_dangerous_path_components() {
        let bad_components: [&[u8]; 8] = [b"..", b".", b"", b"a/b", b"a\\b", b"a\0b", b"C:", b"z:"];
        for bad_component in bad_components {
            let bytes = multifile(b"release", vec![(vec![b"dir", bad_component], 10)]);
            assert!(
                matches!(
                    validate(&bytes, None),
                    Err(Error::InvalidPathComponent { .. })
                ),
                "component {bad_component:?} must be rejected"
            );
        }

        // The torrent name becomes the output subfolder: same rules.
        let bad_names: [&[u8]; 5] = [b"..", b".", b"a/b", b"a\\b", b""];
        for bad_name in bad_names {
            let bytes = multifile(bad_name, vec![(vec![b"f"], 10)]);
            assert!(
                matches!(
                    validate(&bytes, None),
                    Err(Error::InvalidPathComponent { .. })
                ),
                "name {bad_name:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_symlink_entries() {
        // File dict with a "symlink path" key (keys stay sorted).
        let file = bdict(vec![
            (b"length", bint(10)),
            (b"path", blist(vec![bstr(b"f")])),
            (b"symlink path", blist(vec![bstr(b"etc"), bstr(b"passwd")])),
        ]);
        let info = bdict(vec![
            (b"files", blist(vec![file])),
            (b"name", bstr(b"release")),
            (b"piece length", bint(16_384)),
            (b"pieces", bstr(&[0u8; 20])),
        ]);
        let bytes = torrent_bytes(info);
        assert!(matches!(
            validate(&bytes, None),
            Err(Error::InvalidPathComponent { reason, .. }) if reason.contains("symlink")
        ));
    }

    #[test]
    fn rejects_duplicate_file_paths() {
        let bytes = multifile(
            b"release",
            vec![(vec![b"a", b"b"], 10), (vec![b"a", b"b"], 20)],
        );
        assert!(matches!(
            validate(&bytes, None),
            Err(Error::DuplicateFilePath(_))
        ));

        // Same-length but different paths are fine.
        let ok = multifile(
            b"release",
            vec![(vec![b"a", b"b"], 10), (vec![b"a", b"c"], 10)],
        );
        validate(&ok, None).unwrap();
    }

    #[test]
    fn rejects_file_vs_directory_prefix_collisions() {
        // "a" as a file and "a" as a directory of another file.
        let bytes = multifile(b"release", vec![(vec![b"a"], 10), (vec![b"a", b"b"], 20)]);
        assert!(matches!(
            validate(&bytes, None),
            Err(Error::PrefixPathCollision { .. })
        ));

        // Same collision, declared in reverse order.
        let bytes = multifile(b"release", vec![(vec![b"a", b"b"], 20), (vec![b"a"], 10)]);
        assert!(matches!(
            validate(&bytes, None),
            Err(Error::PrefixPathCollision {
                ref directory,
                ref file
            }) if directory == "a" && file == "a/b"
        ));

        // Shared prefixes that are not strict ancestor paths are fine.
        let ok = multifile(b"release", vec![(vec![b"ab"], 10), (vec![b"a", b"b"], 20)]);
        validate(&ok, None).unwrap();
    }

    #[test]
    fn rejects_non_portable_components() {
        let bad_components: [&[u8]; 11] = [
            b"\xff\xfe", // not UTF-8
            b"a\x01b",   // ASCII control
            b"a\x7fb",   // DEL
            b"file.",    // trailing dot
            b"file ",    // trailing space
            b"CON",      // reserved device name
            b"con.txt",  // reserved with extension
            b"Com1",     // reserved serial port, mixed case
            b"LPT9.md",  // reserved line-printer port with extension
            b"nul",      // reserved, lowercase
            b"a/b",      // separator (kept from earlier rules)
        ];
        for bad in bad_components {
            let bytes = multifile(b"release", vec![(vec![b"dir", bad], 10)]);
            assert!(
                matches!(
                    validate(&bytes, None),
                    Err(Error::InvalidPathComponent { .. })
                ),
                "component {bad:?} must be rejected"
            );
        }

        // Portable lookalikes must be accepted.
        let ok = multifile(
            b"release",
            vec![
                (vec!["conquest.txt".as_bytes()], 10),
                (vec!["COM10".as_bytes()], 10), // COM10 is not in COM1-9
                (vec![".hidden".as_bytes()], 10),
                (vec!["trail.ing".as_bytes()], 10), // inner dot is fine
                (vec!["ünïcödé 名前".as_bytes()], 10),
            ],
        );
        validate(&ok, None).unwrap();
    }

    #[test]
    fn rejects_empty_content() {
        let single = torrent_bytes(singlefile_info(b"empty.bin", 0));
        assert!(matches!(validate(&single, None), Err(Error::EmptyContent)));

        let multi = multifile(b"release", vec![(vec![b"a"], 0), (vec![b"b"], 0)]);
        assert!(matches!(validate(&multi, None), Err(Error::EmptyContent)));
    }

    #[test]
    fn rejects_length_overflow() {
        // Each length is the largest bencode-representable positive value;
        // their sum overflows u64.
        let max = i64::MAX as u64;
        let bytes = multifile(
            b"release",
            vec![(vec![b"a"], max), (vec![b"b"], max), (vec![b"c"], 2)],
        );
        assert!(matches!(
            validate(&bytes, None),
            Err(Error::InvalidMetainfo(_))
        ));
    }

    #[test]
    fn enforces_numeric_limits() {
        let limits = MetainfoLimits {
            max_files: 2,
            max_total_bytes: 1000,
            max_path_components: 2,
            max_component_bytes: 4,
            max_path_bytes: 8,
            ..Default::default()
        };

        // File count.
        let bytes = multifile(
            b"r",
            vec![(vec![b"a"], 1), (vec![b"b"], 1), (vec![b"c"], 1)],
        );
        assert!(matches!(
            validate(&bytes, Some(&limits)),
            Err(Error::LimitExceeded {
                limit: "file count",
                value: 3,
                max: 2
            })
        ));

        // Total bytes.
        let bytes = multifile(b"r", vec![(vec![b"a"], 999), (vec![b"b"], 2)]);
        assert!(matches!(
            validate(&bytes, Some(&limits)),
            Err(Error::LimitExceeded {
                limit: "total content bytes",
                value: 1001,
                max: 1000
            })
        ));

        // Path depth.
        let bytes = multifile(b"r", vec![(vec![b"a", b"b", b"c"], 1)]);
        assert!(matches!(
            validate(&bytes, Some(&limits)),
            Err(Error::LimitExceeded {
                limit: "path components per file",
                value: 3,
                max: 2
            })
        ));

        // Component length.
        let bytes = multifile(b"r", vec![(vec![b"abcde"], 1)]);
        assert!(matches!(
            validate(&bytes, Some(&limits)),
            Err(Error::LimitExceeded {
                limit: "path component bytes",
                value: 5,
                max: 4
            })
        ));

        // Whole relative path length ("ab" + "/" + "cde" = 6 > 5 via name... use max 5).
        let limits = MetainfoLimits {
            max_path_bytes: 5,
            ..limits
        };
        let bytes = multifile(b"r", vec![(vec![b"ab", b"cde"], 1)]);
        assert!(matches!(
            validate(&bytes, Some(&limits)),
            Err(Error::LimitExceeded {
                limit: "relative path bytes",
                value: 6,
                max: 5
            })
        ));
    }
}
