//! Minimal Flattened Device Tree (DTB) parser for SemOS aarch64.
//!
//! m1n1 hands the kernel a physical pointer to the DTB. After the MMU has
//! been enabled with an identity map, that pointer can be read directly as a
//! byte slice. This module parses the header, walks the structure block, and
//! exposes a few common queries (memory layout, compatible nodes, stdout UART)
//! used by the platform layer.
//!
//! No allocations are performed; all reads are bounds-checked against the
//! totalsize field in the DTB header.

// FDT header field offsets.
const OFS_TOTALSIZE: usize = 4;
const OFS_OFF_DT_STRUCT: usize = 8;
const OFS_OFF_DT_STRINGS: usize = 12;
const OFS_OFF_MEM_RSVMAP: usize = 16;
const OFS_VERSION: usize = 20;
const OFS_SIZE_DT_STRINGS: usize = 32;
const OFS_SIZE_DT_STRUCT: usize = 36;

const FDT_MAGIC: u32 = 0xd00dfeed;

// Structure block tokens.
const FDT_BEGIN_NODE: u32 = 0x00000001;
const FDT_END_NODE: u32 = 0x00000002;
const FDT_PROP: u32 = 0x00000003;
const FDT_NOP: u32 = 0x00000004;
const FDT_END: u32 = 0x00000009;

const MAX_DEPTH: usize = 64;
const MAX_PROPS: usize = 32;
const MAX_PATH_SEGMENTS: usize = 16;

/// A parsed, bounds-checked view of a DTB.
#[derive(Clone, Copy)]
pub struct Fdt {
    blob: &'static [u8],
    off_dt_struct: usize,
    off_dt_strings: usize,
    off_mem_rsvmap: usize,
    version: u32,
    size_dt_struct: usize,
    size_dt_strings: usize,
}

/// A node discovered during FDT traversal.
#[derive(Clone, Copy)]
pub struct Node {
    /// First `reg` entry of the node, decoded with the parent's address/size
    /// cells.
    pub reg: Option<(u64, u64)>,
    /// Raw value of the node's `compatible` property (may contain multiple
    /// null-terminated compatible strings).
    compatible: Option<&'static [u8]>,
}

impl Node {
    /// Return the raw `compatible` property bytes, if present.
    pub fn compatible_bytes(&self) -> Option<&[u8]> {
        self.compatible
    }

    /// Return the first null-terminated string from the `compatible` property
    /// as a UTF-8 `&str`, if present and valid.
    pub fn compatible_str(&self) -> Option<&str> {
        self.compatible.and_then(|b| {
            let len = b.iter().position(|&x| x == 0).unwrap_or(b.len());
            core::str::from_utf8(&b[..len]).ok()
        })
    }

    /// True if any of the node's `compatible` strings equals `compat`. A node
    /// lists several, most-specific first (Apple's UART is both
    /// `apple,t8103-uart` and `apple,s5l-uart`), so matching only the first
    /// would miss the generic name a driver binds to.
    pub fn is_compatible(&self, compat: &str) -> bool {
        match self.compatible {
            Some(v) => v.split(|&b| b == 0).any(|s| s == compat.as_bytes()),
            None => false,
        }
    }
}

impl Fdt {
    /// Parse a DTB located at the given physical (identity-mapped) pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid DTB that remains readable for the lifetime
    /// of the returned `Fdt` (the kernel identity-maps it as `'static` memory).
    pub unsafe fn from_ptr(ptr: *const u8) -> Option<Fdt> {
        if ptr.is_null() {
            return None;
        }
        // Read the magic and totalsize first, then slice the whole blob.
        let mini = core::slice::from_raw_parts(ptr, 8);
        let magic = read_be32(mini, 0)?;
        if magic != FDT_MAGIC {
            return None;
        }
        let totalsize = read_be32(mini, OFS_TOTALSIZE)? as usize;
        if totalsize < 8 {
            return None;
        }
        let blob = core::slice::from_raw_parts(ptr, totalsize);

        let off_dt_struct = read_be32(blob, OFS_OFF_DT_STRUCT)? as usize;
        let off_dt_strings = read_be32(blob, OFS_OFF_DT_STRINGS)? as usize;
        let off_mem_rsvmap = read_be32(blob, OFS_OFF_MEM_RSVMAP)? as usize;
        let version = read_be32(blob, OFS_VERSION)?;

        if off_dt_struct > totalsize || off_dt_strings > totalsize || off_mem_rsvmap > totalsize {
            return None;
        }

        let size_dt_strings = if version >= 17 {
            let s = read_be32(blob, OFS_SIZE_DT_STRINGS)? as usize;
            if s > 0 {
                s
            } else {
                totalsize - off_dt_strings
            }
        } else if off_dt_struct > off_dt_strings {
            off_dt_struct - off_dt_strings
        } else {
            totalsize - off_dt_strings
        };

        let size_dt_struct = if version >= 17 {
            let s = read_be32(blob, OFS_SIZE_DT_STRUCT)? as usize;
            if s > 0 {
                s
            } else {
                totalsize - off_dt_struct
            }
        } else {
            totalsize - off_dt_struct
        };

        Some(Fdt {
            blob,
            off_dt_struct,
            off_dt_strings,
            off_mem_rsvmap,
            version,
            size_dt_struct,
            size_dt_strings,
        })
    }

    /// Total size of the DTB blob, including the header.
    pub fn totalsize(&self) -> usize {
        self.blob.len()
    }

    /// Return the `(base, size)` of the first `/memory` node `reg` entry.
    pub fn memory(&self) -> Option<(u64, u64)> {
        let mut result = None;
        self.walk_nodes(|name, depth, props, parent_cells| {
            if depth == 2 && (name_eq(name, "memory") || name.starts_with("memory@".as_bytes())) {
                for &(pname, pvalue) in props {
                    if name_eq(pname, "reg") {
                        result = decode_cells(pvalue, parent_cells.0, parent_cells.1);
                        break;
                    }
                }
                false
            } else {
                true
            }
        });
        result
    }

    /// Find the first node whose `compatible` property contains `compat`.
    /// Returns a `Node` with the first `reg` tuple and the raw `compatible`
    /// property.
    pub fn find_compatible(&self, compat: &str) -> Option<Node> {
        let compat_bytes = compat.as_bytes();
        let mut result = None;
        self.walk_nodes(|_name, _depth, props, parent_cells| {
            for &(pname, pvalue) in props {
                if name_eq(pname, "compatible") {
                    for sub in pvalue.split(|&b| b == 0) {
                        if sub == compat_bytes {
                            let reg = props
                                .iter()
                                .find(|&(n, _)| name_eq(n, "reg"))
                                .and_then(|&(_, v)| decode_cells(v, parent_cells.0, parent_cells.1));
                            result = Some(Node {
                                reg,
                                compatible: Some(pvalue),
                            });
                            return false;
                        }
                    }
                }
            }
            true
        });
        result
    }

    /// Return the console UART node selected by `/chosen/stdout-path`, falling
    /// back to the first node compatible with a UART we know how to drive.
    ///
    /// The caller needs the whole node, not just its address: which register
    /// layout to use is decided by `compatible` (Apple's s5l UART and the
    /// PL011 share nothing but the concept of a byte).
    pub fn stdout_uart(&self) -> Option<Node> {
        // 1. Read /chosen stdout-path.
        let mut stdout_prop: Option<&[u8]> = None;
        self.walk_nodes(|name, depth, props, _parent| {
            if depth == 2 && name_eq(name, "chosen") {
                for &(pname, pvalue) in props {
                    if name_eq(pname, "stdout-path") {
                        stdout_prop = Some(pvalue);
                        break;
                    }
                }
                false
            } else {
                true
            }
        });

        if let Some(raw) = stdout_prop {
            if let Some(path) = prop_str(raw) {
                let path = path.split(':').next().unwrap_or(path);
                if !path.is_empty() {
                    let target = if path.starts_with('/') {
                        Some(path)
                    } else {
                        // Resolve a /aliases reference.
                        let alias = path.as_bytes();
                        let mut resolved: Option<&[u8]> = None;
                        self.walk_nodes(|name, depth, props, _parent| {
                            if depth == 2 && name_eq(name, "aliases") {
                                for &(pname, pvalue) in props {
                                    if pname == alias {
                                        resolved = Some(pvalue);
                                        break;
                                    }
                                }
                            }
                            true
                        });
                        resolved.and_then(prop_str)
                    };

                    if let Some(t) = target {
                        if let Some(node) = self.node_by_path(t) {
                            if node.reg.is_some() {
                                return Some(node);
                            }
                        }
                    }
                }
            }
        }

        // 2. Fallback: no usable stdout-path — take the first UART node whose
        //    `compatible` we recognize. Apple first: on a Mac both this and the
        //    stdout-path lookup should agree, and if the tree is missing
        //    /chosen we still want the real console rather than nothing.
        for compat in ["apple,s5l-uart", "arm,pl011"] {
            if let Some(node) = self.find_compatible(compat) {
                if node.reg.is_some() {
                    return Some(node);
                }
            }
        }

        None
    }
}

// ---- private helpers --------------------------------------------------------

impl Fdt {
    /// Walk the structure block, invoking `cb` for every node (including the
    /// root). The callback receives the node name, nesting depth (root == 1),
    /// the node's properties, and the parent address/size cells used to decode
    /// this node's `reg`.
    fn walk_nodes<F>(&self, mut cb: F)
    where
        F: FnMut(&'static [u8], usize, &[(&'static [u8], &'static [u8])], (u32, u32)) -> bool,
    {
        let mut parent_cells: [(u32, u32); MAX_DEPTH] = [(2, 2); MAX_DEPTH];
        let mut depth: usize = 0;
        let mut pos = self.off_dt_struct;
        let struct_end = self
            .off_dt_struct
            .saturating_add(self.size_dt_struct)
            .min(self.blob.len());

        loop {
            if pos.wrapping_add(4) > struct_end {
                return;
            }
            let token = match read_be32(self.blob, pos) {
                Some(t) => t,
                None => return,
            };

            match token {
                FDT_END => return,
                FDT_NOP => {
                    pos += 4;
                }
                FDT_END_NODE => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    pos += 4;
                }
                FDT_BEGIN_NODE => {
                    depth += 1;
                    if depth >= MAX_DEPTH {
                        return;
                    }
                    let (name_end, name) = match read_cstr(self.blob, pos + 4) {
                        Some(x) => x,
                        None => return,
                    };
                    pos = align4(name_end);

                    let mut props: [(&'static [u8], &'static [u8]); MAX_PROPS] =
                        [(&b""[..], &b""[..]); MAX_PROPS];
                    let mut prop_count: usize = 0;
                    let mut addr_cells = parent_cells[depth - 1].0;
                    let mut size_cells = parent_cells[depth - 1].1;
                    let mut done = false;

                    while pos.wrapping_add(4) <= struct_end {
                        let tok = match read_be32(self.blob, pos) {
                            Some(t) => t,
                            None => return,
                        };
                        if tok == FDT_NOP {
                            pos += 4;
                            continue;
                        }
                        // Leave the token for the outer loop, which owns `depth`
                        // bookkeeping — consuming it here would skip `depth -= 1`.
                        if tok == FDT_END_NODE {
                            done = true;
                            break;
                        }
                        if tok == FDT_BEGIN_NODE {
                            done = true;
                            break;
                        }
                        if tok == FDT_PROP {
                            let strings_end = self
                                .off_dt_strings
                                .saturating_add(self.size_dt_strings)
                                .min(self.blob.len());
                            let (pname, pvalue, new_pos) = match parse_prop(
                                self.blob,
                                pos,
                                self.off_dt_strings,
                                strings_end,
                            ) {
                                Some(x) => x,
                                None => return,
                            };
                            pos = new_pos;
                            if prop_count < MAX_PROPS {
                                props[prop_count] = (pname, pvalue);
                                prop_count += 1;
                            } else {
                                return;
                            }
                            if name_eq(pname, "#address-cells") {
                                addr_cells = decode_u32(pvalue);
                            } else if name_eq(pname, "#size-cells") {
                                size_cells = decode_u32(pvalue);
                            }
                            continue;
                        }
                        // Invalid token or premature FDT_END inside a node.
                        return;
                    }
                    if !done {
                        return;
                    }

                    parent_cells[depth] = (addr_cells, size_cells);
                    let cont = cb(name, depth, &props[..prop_count], parent_cells[depth - 1]);
                    if !cont {
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    /// Find a node by its absolute path (e.g., "/serial@9000000"). The path
    /// must start with '/' and use node names as they appear in the DTB.
    fn node_by_path(&self, path: &str) -> Option<Node> {
        let mut segs: [&str; MAX_PATH_SEGMENTS] = [""; MAX_PATH_SEGMENTS];
        let mut count: usize = 0;
        for seg in path.split('/') {
            if seg.is_empty() {
                continue;
            }
            if count < MAX_PATH_SEGMENTS {
                segs[count] = seg;
                count += 1;
            } else {
                return None;
            }
        }
        if count == 0 {
            return None;
        }

        let mut matched: [bool; MAX_DEPTH] = [false; MAX_DEPTH];
        matched[1] = true; // root is at depth 1 and always matches

        let mut result: Option<Node> = None;
        self.walk_nodes(|name, depth, props, parent_cells| {
            if depth >= MAX_DEPTH {
                return true;
            }
            if depth == 1 {
                matched[depth] = true;
                return true;
            }
            if depth.saturating_sub(2) >= count {
                matched[depth] = false;
                return true;
            }
            let expected = segs[depth - 2];
            let is_match = matched[depth - 1] && name == expected.as_bytes();
            matched[depth] = is_match;
            if depth == count + 1 && is_match {
                let reg = props
                    .iter()
                    .find(|&(n, _)| name_eq(n, "reg"))
                    .and_then(|&(_, v)| decode_cells(v, parent_cells.0, parent_cells.1));
                let compatible = props
                    .iter()
                    .find(|&(n, _)| name_eq(n, "compatible"))
                    .map(|&(_, v)| v);
                result = Some(Node { reg, compatible });
                false
            } else {
                true
            }
        });
        result
    }
}

// ---- low-level parsing ------------------------------------------------------

const fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn read_be32(blob: &[u8], off: usize) -> Option<u32> {
    if off.wrapping_add(4) > blob.len() {
        return None;
    }
    Some(u32::from_be_bytes([
        blob[off],
        blob[off + 1],
        blob[off + 2],
        blob[off + 3],
    ]))
}

/// Read a null-terminated string from `blob`. Returns the offset *after* the
/// null byte and the string bytes (excluding the terminator).
fn read_cstr(blob: &[u8], start: usize) -> Option<(usize, &[u8])> {
    let mut end = start;
    while end < blob.len() {
        if blob[end] == 0 {
            return Some((end + 1, &blob[start..end]));
        }
        end += 1;
    }
    None
}

/// Parse a property token at `off`. Returns `(name, value, offset_after_prop)`.
fn parse_prop(
    blob: &[u8],
    off: usize,
    strings_off: usize,
    strings_end: usize,
) -> Option<(&[u8], &[u8], usize)> {
    // `off` is the FDT_PROP token itself; len/nameoff/value follow it.
    let len = read_be32(blob, off + 4)? as usize;
    let nameoff = read_be32(blob, off + 8)? as usize;
    let name_start = strings_off.checked_add(nameoff)?;
    if name_start >= strings_end || name_start >= blob.len() {
        return None;
    }
    let (name_end, name) = read_cstr(blob, name_start)?;
    if name_end > strings_end || name_end > blob.len() {
        return None;
    }
    let value_start = off.checked_add(12)?;
    let value_end = value_start.checked_add(len)?;
    if value_end > blob.len() {
        return None;
    }
    let value = &blob[value_start..value_end];
    Some((name, value, align4(value_end)))
}

fn decode_u32(value: &[u8]) -> u32 {
    if value.len() >= 4 {
        u32::from_be_bytes([value[0], value[1], value[2], value[3]])
    } else {
        0
    }
}

fn decode_cells(value: &[u8], address_cells: u32, size_cells: u32) -> Option<(u64, u64)> {
    let ac = address_cells as usize;
    let sc = size_cells as usize;
    if ac > 2 || sc > 2 {
        return None;
    }
    let pair_len = ac.checked_add(sc)?.checked_mul(4)?;
    if value.len() < pair_len {
        return None;
    }
    let mut base = 0u64;
    let mut off = 0usize;
    for _ in 0..ac {
        base = (base << 32) | (read_be32(value, off)? as u64);
        off += 4;
    }
    let mut size = 0u64;
    for _ in 0..sc {
        size = (size << 32) | (read_be32(value, off)? as u64);
        off += 4;
    }
    Some((base, size))
}

/// Convert a string property value to a `&str`, stripping the trailing null
/// and surrounding whitespace.
fn prop_str(value: &[u8]) -> Option<&str> {
    let len = value.iter().position(|&b| b == 0).unwrap_or(value.len());
    core::str::from_utf8(&value[..len]).ok().map(|s| s.trim())
}

fn name_eq(name: &[u8], s: &str) -> bool {
    name == s.as_bytes()
}