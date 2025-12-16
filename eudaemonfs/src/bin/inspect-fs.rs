//! Filesystem inspector for debugging eudaemonfs at 2am.
//!
//! This tool provides a Lisp REPL with low-level access to filesystem structures
//! for debugging corrupt or broken filesystems. It operates directly on disk files
//! without loading the full filesystem machinery, making it safe to use on damaged images.
//!
//! # Usage
//!
//! ```text
//! inspect-fs <disk-image>
//! ```
//!
//! Then use Lisp expressions at the interactive prompt:
//!
//! - `(superblock)` - Display superblock information
//! - `(block <addr>)` - Read and display a raw block
//! - `(inode <num>)` - Read and display an inode
//! - `(dir <inode>)` - Dump directory contents
//! - `(path <path>)` - Resolve a path and show inode info
//! - `(block-type <addr>)` - Identify what type of block this is
//! - `(owner <addr>)` - Find which inode owns a block
//! - `(blocks <inode>)` - List all blocks owned by an inode
//! - `(log-status)` - Show log head/tail status
//! - `(scan)` - Scan the inode map and build index
//! - `(check)` - Run corruption checks
//! - `(tree [path])` - Show directory tree from path
//! - `(hexdump <addr> [offset] [len])` - Hexdump a block
//! - `(help)` - Show help

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::sync::Arc;
use std::sync::Mutex;

use lispdown::Parser;
use lispdown::SExpr;
use lispdown::Vm;
use rustyline::Context;
use rustyline::EditMode;
use rustyline::Editor;
use rustyline::Helper;
use rustyline::completion::Completer;
use rustyline::completion::Pair;
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::highlight::CmdKind;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::ValidationContext;
use rustyline::validate::ValidationResult;
use rustyline::validate::Validator;

////////////////////////////////////////////// Constants /////////////////////////////////////////////

/// Block size in bytes.
const BLOCK_SIZE: usize = 4096;

/// Magic number for the superblock.
const MAGIC: u64 = 0x4C46535F53594E46; // "LFS_SYNF"

/// Maximum filename length.
const MAX_FILENAME_LEN: usize = 255;

/// Number of direct block pointers in an inode.
const DIRECT_BLOCKS: usize = 9;

/// Number of block pointers per indirect block.
const PTRS_PER_BLOCK: usize = BLOCK_SIZE / 8;

/// Size of a directory entry in bytes.
const DIR_ENTRY_SIZE: usize = 272;

/// Number of directory entries per block.
const DIR_ENTRIES_PER_BLOCK: usize = BLOCK_SIZE / DIR_ENTRY_SIZE;

/// Invalid block address marker.
const INVALID_BLOCK: u64 = u64::MAX;

/// Invalid inode number.
const INVALID_INODE: u64 = 0;

/// Root inode number.
const ROOT_INODE: u64 = 1;

////////////////////////////////////////////// Superblock ////////////////////////////////////////////

/// Parsed superblock structure.
#[derive(Debug, Clone)]
struct Superblock {
    magic: u64,
    block_size: u32,
    total_blocks: u64,
    log_start: u64,
    log_end: u64,
    head: u64,
    tail: u64,
    inode_map_block: u64,
    next_inode: u64,
}

impl Superblock {
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 72 {
            return None;
        }
        Some(Self {
            magic: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            block_size: u32::from_le_bytes(buf[8..12].try_into().ok()?),
            total_blocks: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            log_start: u64::from_le_bytes(buf[24..32].try_into().ok()?),
            log_end: u64::from_le_bytes(buf[32..40].try_into().ok()?),
            head: u64::from_le_bytes(buf[40..48].try_into().ok()?),
            tail: u64::from_le_bytes(buf[48..56].try_into().ok()?),
            inode_map_block: u64::from_le_bytes(buf[56..64].try_into().ok()?),
            next_inode: u64::from_le_bytes(buf[64..72].try_into().ok()?),
        })
    }

    fn is_valid(&self) -> bool {
        self.magic == MAGIC
    }

    fn to_sexpr(&self) -> SExpr {
        let valid = if self.is_valid() { "true" } else { "false" };
        let log_size = self.log_end - self.log_start;
        let used = if self.tail >= self.head {
            self.tail - self.head
        } else {
            (self.log_end - self.head) + (self.tail - self.log_start)
        };
        let usage_pct = if log_size > 0 {
            (used * 100) / log_size
        } else {
            0
        };

        SExpr::List(vec![
            SExpr::Atom("superblock".to_string()),
            SExpr::List(vec![
                SExpr::Atom("magic".to_string()),
                SExpr::Atom(format!("0x{:016x}", self.magic)),
            ]),
            SExpr::List(vec![
                SExpr::Atom("valid".to_string()),
                SExpr::Atom(valid.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("block-size".to_string()),
                SExpr::Atom(self.block_size.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("total-blocks".to_string()),
                SExpr::Atom(self.total_blocks.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("log-start".to_string()),
                SExpr::Atom(self.log_start.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("log-end".to_string()),
                SExpr::Atom(self.log_end.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("log-size".to_string()),
                SExpr::Atom(log_size.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("head".to_string()),
                SExpr::Atom(self.head.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("tail".to_string()),
                SExpr::Atom(self.tail.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("inode-map-block".to_string()),
                SExpr::Atom(if self.inode_map_block == INVALID_BLOCK {
                    "none".to_string()
                } else {
                    self.inode_map_block.to_string()
                }),
            ]),
            SExpr::List(vec![
                SExpr::Atom("next-inode".to_string()),
                SExpr::Atom(self.next_inode.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("used-blocks".to_string()),
                SExpr::Atom(used.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("usage-percent".to_string()),
                SExpr::Atom(usage_pct.to_string()),
            ]),
        ])
    }
}

/////////////////////////////////////////////// Inode ////////////////////////////////////////////////

/// Inode type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InodeType {
    File,
    Directory,
    Symlink,
    Unknown(u8),
}

impl InodeType {
    fn from_u8(val: u8) -> Self {
        match val {
            0 => InodeType::File,
            1 => InodeType::Directory,
            2 => InodeType::Symlink,
            n => InodeType::Unknown(n),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            InodeType::File => "file",
            InodeType::Directory => "directory",
            InodeType::Symlink => "symlink",
            InodeType::Unknown(_) => "unknown",
        }
    }
}

/// Parsed inode structure.
#[derive(Debug, Clone)]
struct Inode {
    ino: u64,
    inode_type: InodeType,
    link_count: u32,
    size: u64,
    atime_ms: i64,
    mtime_ms: i64,
    direct: [u64; DIRECT_BLOCKS],
    indirect: u64,
    double_indirect: u64,
}

impl Inode {
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 128 {
            return None;
        }
        let mut direct = [INVALID_BLOCK; DIRECT_BLOCKS];
        for (i, block) in direct.iter_mut().enumerate() {
            let offset = 40 + i * 8;
            *block = u64::from_le_bytes(buf[offset..offset + 8].try_into().ok()?);
        }
        Some(Self {
            ino: u64::from_le_bytes(buf[0..8].try_into().ok()?),
            inode_type: InodeType::from_u8(buf[8]),
            link_count: u32::from_le_bytes(buf[9..13].try_into().ok()?),
            size: u64::from_le_bytes(buf[16..24].try_into().ok()?),
            atime_ms: i64::from_le_bytes(buf[24..32].try_into().ok()?),
            mtime_ms: i64::from_le_bytes(buf[32..40].try_into().ok()?),
            direct,
            indirect: u64::from_le_bytes(buf[112..120].try_into().ok()?),
            double_indirect: u64::from_le_bytes(buf[120..128].try_into().ok()?),
        })
    }

    fn to_sexpr(&self) -> SExpr {
        let mut items = vec![
            SExpr::Atom("inode".to_string()),
            SExpr::List(vec![
                SExpr::Atom("ino".to_string()),
                SExpr::Atom(self.ino.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom(self.inode_type.as_str().to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("link-count".to_string()),
                SExpr::Atom(self.link_count.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("size".to_string()),
                SExpr::Atom(self.size.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("atime-ms".to_string()),
                SExpr::Atom(self.atime_ms.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("mtime-ms".to_string()),
                SExpr::Atom(self.mtime_ms.to_string()),
            ]),
        ];

        // Add direct blocks
        let direct_blocks: Vec<SExpr> = self
            .direct
            .iter()
            .enumerate()
            .filter(|(_, b)| **b != INVALID_BLOCK)
            .map(|(i, &b)| {
                SExpr::List(vec![SExpr::Atom(i.to_string()), SExpr::Atom(b.to_string())])
            })
            .collect();
        if !direct_blocks.is_empty() {
            let mut direct_list = vec![SExpr::Atom("direct".to_string())];
            direct_list.extend(direct_blocks);
            items.push(SExpr::List(direct_list));
        }

        if self.indirect != INVALID_BLOCK {
            items.push(SExpr::List(vec![
                SExpr::Atom("indirect".to_string()),
                SExpr::Atom(self.indirect.to_string()),
            ]));
        }

        if self.double_indirect != INVALID_BLOCK {
            items.push(SExpr::List(vec![
                SExpr::Atom("double-indirect".to_string()),
                SExpr::Atom(self.double_indirect.to_string()),
            ]));
        }

        SExpr::List(items)
    }

    fn is_directory(&self) -> bool {
        self.inode_type == InodeType::Directory
    }
}

////////////////////////////////////////////// DirEntry //////////////////////////////////////////////

/// Parsed directory entry.
#[derive(Debug, Clone)]
struct DirEntry {
    ino: u64,
    name_len: u16,
    name: String,
}

impl DirEntry {
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < DIR_ENTRY_SIZE {
            return None;
        }
        let ino = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let name_len = u16::from_le_bytes(buf[8..10].try_into().ok()?);
        let name_bytes = &buf[10..10 + (name_len as usize).min(MAX_FILENAME_LEN)];
        let name = String::from_utf8_lossy(name_bytes).to_string();
        Some(Self {
            ino,
            name_len,
            name,
        })
    }

    fn is_valid(&self) -> bool {
        self.ino != INVALID_INODE
    }

    fn to_sexpr(&self) -> SExpr {
        SExpr::List(vec![
            SExpr::Atom("entry".to_string()),
            SExpr::List(vec![
                SExpr::Atom("ino".to_string()),
                SExpr::Atom(self.ino.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("name".to_string()),
                SExpr::Atom(format!("\"{}\"", self.name)),
            ]),
        ])
    }
}

////////////////////////////////////////////// Inspector /////////////////////////////////////////////

/// The filesystem inspector state.
struct Inspector {
    file: File,
    superblock: Option<Superblock>,
    /// Inode map: inode number -> block address
    inode_map: BTreeMap<u64, u64>,
    /// Reverse map: block address -> (inode, block_index, entry_type)
    block_owners: BTreeMap<u64, (u64, u64, String)>,
    /// Whether we've scanned the filesystem
    scanned: bool,
}

impl Inspector {
    fn new(path: &str) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let mut inspector = Self {
            file,
            superblock: None,
            inode_map: BTreeMap::new(),
            block_owners: BTreeMap::new(),
            scanned: false,
        };
        inspector.load_superblock()?;
        Ok(inspector)
    }

    fn read_block(&mut self, block: u64) -> std::io::Result<[u8; BLOCK_SIZE]> {
        let mut buf = [0u8; BLOCK_SIZE];
        let offset = block * BLOCK_SIZE as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn load_superblock(&mut self) -> std::io::Result<()> {
        let block = self.read_block(0)?;
        self.superblock = Superblock::from_bytes(&block);
        Ok(())
    }

    fn read_inode_at_block(&mut self, block: u64) -> std::io::Result<Option<Inode>> {
        let buf = self.read_block(block)?;
        Ok(Inode::from_bytes(&buf))
    }

    fn read_inode(&mut self, ino: u64) -> std::io::Result<Option<Inode>> {
        if let Some(&block) = self.inode_map.get(&ino) {
            self.read_inode_at_block(block)
        } else {
            Ok(None)
        }
    }

    fn load_inode_map(&mut self) -> std::io::Result<usize> {
        let sb = match &self.superblock {
            Some(sb) => sb.clone(),
            None => return Ok(0),
        };

        let mut block_addr = sb.inode_map_block;
        if block_addr == INVALID_BLOCK {
            return Ok(0);
        }

        let entries_per_block = (BLOCK_SIZE - 8) / 16;
        let mut count = 0;

        while block_addr != INVALID_BLOCK {
            let block = self.read_block(block_addr)?;
            let next_block = u64::from_le_bytes(block[0..8].try_into().unwrap());

            for i in 0..entries_per_block {
                let offset = 8 + i * 16;
                let ino = u64::from_le_bytes(block[offset..offset + 8].try_into().unwrap());
                let addr = u64::from_le_bytes(block[offset + 8..offset + 16].try_into().unwrap());

                if ino != INVALID_INODE && addr != INVALID_BLOCK {
                    self.inode_map.insert(ino, addr);
                    self.block_owners
                        .insert(addr, (ino, 0, "inode".to_string()));
                    count += 1;
                }
            }

            block_addr = next_block;
        }

        Ok(count)
    }

    fn scan(&mut self) -> std::io::Result<(usize, usize)> {
        self.inode_map.clear();
        self.block_owners.clear();

        let inode_count = self.load_inode_map()?;

        // Now scan each inode for its data blocks
        let inodes: Vec<(u64, u64)> = self.inode_map.iter().map(|(&k, &v)| (k, v)).collect();

        for (ino, _block_addr) in inodes {
            if let Some(inode) = self.read_inode(ino)? {
                self.index_inode_blocks(ino, &inode)?;
            }
        }

        self.scanned = true;
        Ok((inode_count, self.block_owners.len()))
    }

    fn index_inode_blocks(&mut self, ino: u64, inode: &Inode) -> std::io::Result<()> {
        // Index direct blocks
        for (i, &block) in inode.direct.iter().enumerate() {
            if block != INVALID_BLOCK {
                self.block_owners
                    .insert(block, (ino, i as u64, "data".to_string()));
            }
        }

        // Index indirect block
        if inode.indirect != INVALID_BLOCK {
            self.block_owners
                .insert(inode.indirect, (ino, 0, "indirect".to_string()));
            let indirect_data = self.read_block(inode.indirect)?;
            for i in 0..PTRS_PER_BLOCK {
                let offset = i * 8;
                let ptr = u64::from_le_bytes(indirect_data[offset..offset + 8].try_into().unwrap());
                if ptr != INVALID_BLOCK {
                    let block_idx = DIRECT_BLOCKS as u64 + i as u64;
                    self.block_owners
                        .insert(ptr, (ino, block_idx, "data".to_string()));
                }
            }
        }

        // Index double indirect block
        if inode.double_indirect != INVALID_BLOCK {
            self.block_owners.insert(
                inode.double_indirect,
                (ino, 0, "double-indirect".to_string()),
            );
            let double_data = self.read_block(inode.double_indirect)?;
            for i in 0..PTRS_PER_BLOCK {
                let offset = i * 8;
                let first_ptr =
                    u64::from_le_bytes(double_data[offset..offset + 8].try_into().unwrap());
                if first_ptr != INVALID_BLOCK {
                    self.block_owners
                        .insert(first_ptr, (ino, i as u64, "indirect-l2".to_string()));
                    let first_data = self.read_block(first_ptr)?;
                    for j in 0..PTRS_PER_BLOCK {
                        let offset2 = j * 8;
                        let ptr = u64::from_le_bytes(
                            first_data[offset2..offset2 + 8].try_into().unwrap(),
                        );
                        if ptr != INVALID_BLOCK {
                            let block_idx = DIRECT_BLOCKS as u64
                                + PTRS_PER_BLOCK as u64
                                + (i * PTRS_PER_BLOCK + j) as u64;
                            self.block_owners
                                .insert(ptr, (ino, block_idx, "data".to_string()));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn get_block_addr(&mut self, inode: &Inode, block_idx: u64) -> std::io::Result<u64> {
        if block_idx < DIRECT_BLOCKS as u64 {
            return Ok(inode.direct[block_idx as usize]);
        }

        let indirect_idx = block_idx - DIRECT_BLOCKS as u64;
        if indirect_idx < PTRS_PER_BLOCK as u64 {
            if inode.indirect == INVALID_BLOCK {
                return Ok(INVALID_BLOCK);
            }
            let block = self.read_block(inode.indirect)?;
            let offset = indirect_idx as usize * 8;
            let ptr = u64::from_le_bytes(block[offset..offset + 8].try_into().unwrap());
            return Ok(ptr);
        }

        let double_idx = indirect_idx - PTRS_PER_BLOCK as u64;
        if double_idx < (PTRS_PER_BLOCK * PTRS_PER_BLOCK) as u64 {
            if inode.double_indirect == INVALID_BLOCK {
                return Ok(INVALID_BLOCK);
            }

            let first_level_idx = double_idx / PTRS_PER_BLOCK as u64;
            let second_level_idx = double_idx % PTRS_PER_BLOCK as u64;

            let double_block = self.read_block(inode.double_indirect)?;
            let first_offset = first_level_idx as usize * 8;
            let first_ptr = u64::from_le_bytes(
                double_block[first_offset..first_offset + 8]
                    .try_into()
                    .unwrap(),
            );

            if first_ptr == INVALID_BLOCK {
                return Ok(INVALID_BLOCK);
            }

            let first_block = self.read_block(first_ptr)?;
            let second_offset = second_level_idx as usize * 8;
            let second_ptr = u64::from_le_bytes(
                first_block[second_offset..second_offset + 8]
                    .try_into()
                    .unwrap(),
            );
            return Ok(second_ptr);
        }

        Ok(INVALID_BLOCK)
    }

    fn dump_directory(&mut self, ino: u64) -> std::io::Result<SExpr> {
        let inode = match self.read_inode(ino)? {
            Some(i) => i,
            None => {
                return Ok(SExpr::List(vec![
                    SExpr::Atom("error".to_string()),
                    SExpr::Atom(format!("inode {} not found", ino)),
                ]));
            }
        };

        if !inode.is_directory() {
            return Ok(SExpr::List(vec![
                SExpr::Atom("error".to_string()),
                SExpr::Atom(format!("inode {} is not a directory", ino)),
            ]));
        }

        let mut entries = vec![SExpr::Atom("directory".to_string())];
        entries.push(SExpr::List(vec![
            SExpr::Atom("ino".to_string()),
            SExpr::Atom(ino.to_string()),
        ]));
        entries.push(SExpr::List(vec![
            SExpr::Atom("entry-count".to_string()),
            SExpr::Atom(inode.size.to_string()),
        ]));

        let mut entry_list = vec![SExpr::Atom("entries".to_string())];
        let num_entries = inode.size;

        for i in 0..num_entries {
            let block_num = i / DIR_ENTRIES_PER_BLOCK as u64;
            let entry_in_block = i % DIR_ENTRIES_PER_BLOCK as u64;

            let block_addr = self.get_block_addr(&inode, block_num)?;
            if block_addr == INVALID_BLOCK {
                entry_list.push(SExpr::List(vec![
                    SExpr::Atom("hole".to_string()),
                    SExpr::Atom(i.to_string()),
                ]));
                continue;
            }

            let block = self.read_block(block_addr)?;
            let entry_offset = (entry_in_block as usize) * DIR_ENTRY_SIZE;

            if let Some(entry) = DirEntry::from_bytes(&block[entry_offset..]) {
                if entry.is_valid() {
                    entry_list.push(entry.to_sexpr());
                } else {
                    entry_list.push(SExpr::List(vec![
                        SExpr::Atom("deleted".to_string()),
                        SExpr::Atom(i.to_string()),
                    ]));
                }
            } else {
                entry_list.push(SExpr::List(vec![
                    SExpr::Atom("corrupt".to_string()),
                    SExpr::Atom(i.to_string()),
                ]));
            }
        }

        entries.push(SExpr::List(entry_list));
        Ok(SExpr::List(entries))
    }

    fn resolve_path(&mut self, path: &str) -> std::io::Result<Option<u64>> {
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Ok(Some(ROOT_INODE));
        }

        let mut current_ino = ROOT_INODE;

        for component in components {
            let inode = match self.read_inode(current_ino)? {
                Some(i) => i,
                None => return Ok(None),
            };

            if !inode.is_directory() {
                return Ok(None);
            }

            let mut found = false;
            let num_entries = inode.size;
            for i in 0..num_entries {
                let block_num = i / DIR_ENTRIES_PER_BLOCK as u64;
                let entry_in_block = i % DIR_ENTRIES_PER_BLOCK as u64;

                let block_addr = self.get_block_addr(&inode, block_num)?;
                if block_addr == INVALID_BLOCK {
                    continue;
                }

                let block = self.read_block(block_addr)?;
                let entry_offset = (entry_in_block as usize) * DIR_ENTRY_SIZE;

                if let Some(entry) = DirEntry::from_bytes(&block[entry_offset..])
                    && entry.is_valid()
                    && entry.name == component
                {
                    current_ino = entry.ino;
                    found = true;
                    break;
                }
            }

            if !found {
                return Ok(None);
            }
        }

        Ok(Some(current_ino))
    }

    fn list_blocks(&mut self, ino: u64) -> std::io::Result<SExpr> {
        let inode = match self.read_inode(ino)? {
            Some(i) => i,
            None => {
                return Ok(SExpr::List(vec![
                    SExpr::Atom("error".to_string()),
                    SExpr::Atom(format!("inode {} not found", ino)),
                ]));
            }
        };

        let num_blocks = if inode.is_directory() {
            if inode.size == 0 {
                0
            } else {
                (inode.size - 1) / DIR_ENTRIES_PER_BLOCK as u64 + 1
            }
        } else if inode.size == 0 {
            0
        } else {
            (inode.size - 1) / BLOCK_SIZE as u64 + 1
        };

        let mut items = vec![SExpr::Atom("blocks".to_string())];
        items.push(SExpr::List(vec![
            SExpr::Atom("ino".to_string()),
            SExpr::Atom(ino.to_string()),
        ]));
        items.push(SExpr::List(vec![
            SExpr::Atom("size".to_string()),
            SExpr::Atom(inode.size.to_string()),
        ]));
        items.push(SExpr::List(vec![
            SExpr::Atom("expected-blocks".to_string()),
            SExpr::Atom(num_blocks.to_string()),
        ]));

        let mut block_list = vec![SExpr::Atom("data".to_string())];
        for i in 0..num_blocks {
            let addr = self.get_block_addr(&inode, i)?;
            if addr != INVALID_BLOCK {
                block_list.push(SExpr::List(vec![
                    SExpr::Atom(i.to_string()),
                    SExpr::Atom(addr.to_string()),
                ]));
            } else {
                block_list.push(SExpr::List(vec![
                    SExpr::Atom(i.to_string()),
                    SExpr::Atom("hole".to_string()),
                ]));
            }
        }
        items.push(SExpr::List(block_list));

        if inode.indirect != INVALID_BLOCK {
            items.push(SExpr::List(vec![
                SExpr::Atom("indirect".to_string()),
                SExpr::Atom(inode.indirect.to_string()),
            ]));
        }
        if inode.double_indirect != INVALID_BLOCK {
            items.push(SExpr::List(vec![
                SExpr::Atom("double-indirect".to_string()),
                SExpr::Atom(inode.double_indirect.to_string()),
            ]));
        }

        Ok(SExpr::List(items))
    }

    fn hexdump_block(&mut self, block: u64, offset: usize, len: usize) -> std::io::Result<SExpr> {
        let data = self.read_block(block)?;
        let end = (offset + len).min(BLOCK_SIZE);

        let mut lines = vec![SExpr::Atom("hexdump".to_string())];
        lines.push(SExpr::List(vec![
            SExpr::Atom("block".to_string()),
            SExpr::Atom(block.to_string()),
        ]));
        lines.push(SExpr::List(vec![
            SExpr::Atom("offset".to_string()),
            SExpr::Atom(offset.to_string()),
        ]));
        lines.push(SExpr::List(vec![
            SExpr::Atom("length".to_string()),
            SExpr::Atom((end - offset).to_string()),
        ]));

        let mut hex_lines = vec![SExpr::Atom("data".to_string())];
        for row_start in (offset..end).step_by(16) {
            let row_end = (row_start + 16).min(end);
            let hex: String = data[row_start..row_end]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");

            let ascii: String = data[row_start..row_end]
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();

            hex_lines.push(SExpr::List(vec![
                SExpr::Atom(format!("{:04x}", row_start)),
                SExpr::Atom(format!("\"{}\"", hex)),
                SExpr::Atom(format!("\"{}\"", ascii)),
            ]));
        }
        lines.push(SExpr::List(hex_lines));

        Ok(SExpr::List(lines))
    }

    fn identify_block(&mut self, block: u64) -> std::io::Result<SExpr> {
        let mut items = vec![SExpr::Atom("block-type".to_string())];
        items.push(SExpr::List(vec![
            SExpr::Atom("block".to_string()),
            SExpr::Atom(block.to_string()),
        ]));

        // Check if we have ownership info
        if let Some((ino, idx, typ)) = self.block_owners.get(&block) {
            items.push(SExpr::List(vec![
                SExpr::Atom("owner".to_string()),
                SExpr::Atom(ino.to_string()),
            ]));
            items.push(SExpr::List(vec![
                SExpr::Atom("index".to_string()),
                SExpr::Atom(idx.to_string()),
            ]));
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom(typ.clone()),
            ]));
            return Ok(SExpr::List(items));
        }

        if self.scanned {
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom("free-or-garbage".to_string()),
            ]));
            return Ok(SExpr::List(items));
        }

        // Try to interpret the block content
        let data = self.read_block(block)?;

        // Check for superblock
        if block == 0
            && let Some(sb) = Superblock::from_bytes(&data)
            && sb.is_valid()
        {
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom("superblock".to_string()),
            ]));
            return Ok(SExpr::List(items));
        }

        // Check for inode
        let type_byte = data[8];
        if let Some(inode) = Inode::from_bytes(&data)
            && inode.ino > 0
            && inode.ino < 1_000_000
            && type_byte <= 2
        {
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom("possible-inode".to_string()),
            ]));
            items.push(SExpr::List(vec![
                SExpr::Atom("ino".to_string()),
                SExpr::Atom(inode.ino.to_string()),
            ]));
            return Ok(SExpr::List(items));
        }

        // Check for directory entries
        if let Some(entry) = DirEntry::from_bytes(&data)
            && entry.is_valid()
            && entry.name_len > 0
            && entry.name_len < 256
            && entry.name.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        {
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom("possible-directory".to_string()),
            ]));
            return Ok(SExpr::List(items));
        }

        // Check if all zeros
        if data.iter().all(|&b| b == 0) {
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom("empty".to_string()),
            ]));
            return Ok(SExpr::List(items));
        }

        // Check for text content
        let printable_count = data
            .iter()
            .filter(|&&b| b.is_ascii_graphic() || b.is_ascii_whitespace())
            .count();
        if printable_count > BLOCK_SIZE * 3 / 4 {
            items.push(SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom("text-data".to_string()),
            ]));
            items.push(SExpr::List(vec![
                SExpr::Atom("printable-percent".to_string()),
                SExpr::Atom((printable_count * 100 / BLOCK_SIZE).to_string()),
            ]));
            return Ok(SExpr::List(items));
        }

        items.push(SExpr::List(vec![
            SExpr::Atom("type".to_string()),
            SExpr::Atom("unknown".to_string()),
        ]));
        items.push(SExpr::List(vec![
            SExpr::Atom("hint".to_string()),
            SExpr::Atom("run (scan) to build index".to_string()),
        ]));
        Ok(SExpr::List(items))
    }

    fn log_status(&self) -> SExpr {
        let sb = match &self.superblock {
            Some(sb) => sb,
            None => {
                return SExpr::List(vec![
                    SExpr::Atom("error".to_string()),
                    SExpr::Atom("no valid superblock".to_string()),
                ]);
            }
        };

        let log_size = sb.log_end - sb.log_start;
        let (used, free) = if sb.tail >= sb.head {
            (sb.tail - sb.head, log_size - (sb.tail - sb.head))
        } else {
            let used = (sb.log_end - sb.head) + (sb.tail - sb.log_start);
            (used, log_size - used)
        };

        // Visual representation
        let width = 60;
        let mut visual = String::new();
        visual.push('[');
        for i in 0..width {
            let block = sb.log_start + (i as u64 * log_size / width as u64);
            let is_live = if sb.tail >= sb.head {
                block >= sb.head && block < sb.tail
            } else {
                block >= sb.head || block < sb.tail
            };
            if block == sb.head {
                visual.push('H');
            } else if block == sb.tail {
                visual.push('T');
            } else if is_live {
                visual.push('#');
            } else {
                visual.push('.');
            }
        }
        visual.push(']');

        SExpr::List(vec![
            SExpr::Atom("log-status".to_string()),
            SExpr::List(vec![
                SExpr::Atom("log-start".to_string()),
                SExpr::Atom(sb.log_start.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("log-end".to_string()),
                SExpr::Atom(sb.log_end.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("log-size".to_string()),
                SExpr::Atom(log_size.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("head".to_string()),
                SExpr::Atom(sb.head.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("tail".to_string()),
                SExpr::Atom(sb.tail.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("used-blocks".to_string()),
                SExpr::Atom(used.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("free-blocks".to_string()),
                SExpr::Atom(free.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("usage-percent".to_string()),
                SExpr::Atom(((used * 100) / log_size).to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("visual".to_string()),
                SExpr::Atom(format!("\"{}\"", visual)),
            ]),
        ])
    }

    fn run_checks(&mut self) -> std::io::Result<SExpr> {
        let mut items = vec![SExpr::Atom("check".to_string())];
        let mut errors: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        // Check superblock
        let sb = match &self.superblock {
            Some(sb) => sb.clone(),
            None => {
                errors.push("No valid superblock found".to_string());
                items.push(SExpr::List(vec![
                    SExpr::Atom("errors".to_string()),
                    SExpr::List(errors.iter().map(|e| SExpr::Atom(e.clone())).collect()),
                ]));
                return Ok(SExpr::List(items));
            }
        };

        if sb.magic != MAGIC {
            errors.push("Invalid magic number".to_string());
        }

        if sb.block_size != BLOCK_SIZE as u32 {
            warnings.push(format!(
                "Block size {} != expected {}",
                sb.block_size, BLOCK_SIZE
            ));
        }

        if sb.head < sb.log_start || sb.head >= sb.log_end {
            errors.push(format!(
                "Head {} outside log region [{}, {})",
                sb.head, sb.log_start, sb.log_end
            ));
        }

        if sb.tail < sb.log_start || sb.tail >= sb.log_end {
            errors.push(format!(
                "Tail {} outside log region [{}, {})",
                sb.tail, sb.log_start, sb.log_end
            ));
        }

        // Check inode map
        if sb.inode_map_block == INVALID_BLOCK {
            warnings.push("No inode map block set".to_string());
        } else if self.inode_map.is_empty() {
            self.load_inode_map()?;
        }

        items.push(SExpr::List(vec![
            SExpr::Atom("inode-count".to_string()),
            SExpr::Atom(self.inode_map.len().to_string()),
        ]));

        // Check for root inode
        if !self.inode_map.contains_key(&ROOT_INODE) {
            errors.push("Root inode (1) not found in inode map".to_string());
        }

        // Check inode numbers vs next_inode
        for &ino in self.inode_map.keys() {
            if ino >= sb.next_inode {
                errors.push(format!("Inode {} >= next_inode {}", ino, sb.next_inode));
            }
        }

        // Check each inode
        let inodes: Vec<u64> = self.inode_map.keys().copied().collect();
        for ino in inodes {
            if let Some(inode) = self.read_inode(ino)? {
                if inode.ino != ino {
                    errors.push(format!("Inode at map[{}] has ino={}", ino, inode.ino));
                }

                for (i, &block) in inode.direct.iter().enumerate() {
                    if block != INVALID_BLOCK && block >= sb.log_end {
                        errors.push(format!(
                            "Inode {} direct[{}]={} out of bounds",
                            ino, i, block
                        ));
                    }
                }
            }
        }

        items.push(SExpr::List(vec![
            SExpr::Atom("error-count".to_string()),
            SExpr::Atom(errors.len().to_string()),
        ]));
        items.push(SExpr::List(vec![
            SExpr::Atom("warning-count".to_string()),
            SExpr::Atom(warnings.len().to_string()),
        ]));

        if !errors.is_empty() {
            items.push(SExpr::List(
                std::iter::once(SExpr::Atom("errors".to_string()))
                    .chain(errors.iter().map(|e| SExpr::Atom(format!("\"{}\"", e))))
                    .collect(),
            ));
        }

        if !warnings.is_empty() {
            items.push(SExpr::List(
                std::iter::once(SExpr::Atom("warnings".to_string()))
                    .chain(warnings.iter().map(|w| SExpr::Atom(format!("\"{}\"", w))))
                    .collect(),
            ));
        }

        let status = if errors.is_empty() && warnings.is_empty() {
            "healthy"
        } else if errors.is_empty() {
            "minor-issues"
        } else {
            "CORRUPT"
        };
        items.push(SExpr::List(vec![
            SExpr::Atom("status".to_string()),
            SExpr::Atom(status.to_string()),
        ]));

        Ok(SExpr::List(items))
    }

    fn show_tree(&mut self, path: &str) -> std::io::Result<SExpr> {
        let ino = match self.resolve_path(path)? {
            Some(i) => i,
            None => {
                return Ok(SExpr::List(vec![
                    SExpr::Atom("error".to_string()),
                    SExpr::Atom(format!("path not found: {}", path)),
                ]));
            }
        };

        self.build_tree(ino, path)
    }

    fn build_tree(&mut self, ino: u64, path: &str) -> std::io::Result<SExpr> {
        let inode = match self.read_inode(ino)? {
            Some(i) => i,
            None => {
                return Ok(SExpr::List(vec![
                    SExpr::Atom("error".to_string()),
                    SExpr::Atom(format!("inode {} not found", ino)),
                ]));
            }
        };

        let name = if path == "/" || path.is_empty() {
            "/".to_string()
        } else {
            path.rsplit('/').next().unwrap_or(path).to_string()
        };

        match inode.inode_type {
            InodeType::Directory => {
                let mut items = vec![
                    SExpr::Atom("dir".to_string()),
                    SExpr::Atom(format!("\"{}\"", name)),
                ];

                // Collect directory entries
                let mut entries: Vec<(String, u64)> = Vec::new();
                let num_entries = inode.size;

                for i in 0..num_entries {
                    let block_num = i / DIR_ENTRIES_PER_BLOCK as u64;
                    let entry_in_block = i % DIR_ENTRIES_PER_BLOCK as u64;

                    let block_addr = self.get_block_addr(&inode, block_num)?;
                    if block_addr == INVALID_BLOCK {
                        continue;
                    }

                    let block = self.read_block(block_addr)?;
                    let entry_offset = (entry_in_block as usize) * DIR_ENTRY_SIZE;

                    if let Some(entry) = DirEntry::from_bytes(&block[entry_offset..])
                        && entry.is_valid()
                        && entry.name != "."
                        && entry.name != ".."
                    {
                        entries.push((entry.name.clone(), entry.ino));
                    }
                }

                entries.sort_by(|a, b| a.0.cmp(&b.0));

                for (child_name, child_ino) in entries {
                    let child_path = if path == "/" {
                        format!("/{}", child_name)
                    } else {
                        format!("{}/{}", path, child_name)
                    };
                    items.push(self.build_tree(child_ino, &child_path)?);
                }

                Ok(SExpr::List(items))
            }
            InodeType::File => Ok(SExpr::List(vec![
                SExpr::Atom("file".to_string()),
                SExpr::Atom(format!("\"{}\"", name)),
                SExpr::Atom(inode.size.to_string()),
            ])),
            InodeType::Symlink => {
                let target = if inode.size > 0 && inode.size < BLOCK_SIZE as u64 {
                    if let Ok(block_addr) = self.get_block_addr(&inode, 0) {
                        if block_addr != INVALID_BLOCK {
                            if let Ok(block) = self.read_block(block_addr) {
                                String::from_utf8_lossy(&block[..inode.size as usize]).to_string()
                            } else {
                                "?".to_string()
                            }
                        } else {
                            "?".to_string()
                        }
                    } else {
                        "?".to_string()
                    }
                } else {
                    "?".to_string()
                };
                Ok(SExpr::List(vec![
                    SExpr::Atom("symlink".to_string()),
                    SExpr::Atom(format!("\"{}\"", name)),
                    SExpr::Atom(format!("\"{}\"", target)),
                ]))
            }
            InodeType::Unknown(t) => Ok(SExpr::List(vec![
                SExpr::Atom("unknown".to_string()),
                SExpr::Atom(format!("\"{}\"", name)),
                SExpr::Atom(t.to_string()),
            ])),
        }
    }
}

////////////////////////////////////////////// Builtins //////////////////////////////////////////////

/// Thread-safe inspector wrapper for use in VM builtins.
type SharedInspector = Arc<Mutex<Inspector>>;

fn get_inspector(vm: &Vm) -> Option<&SharedInspector> {
    vm.get_user_data::<SharedInspector>()
}

fn extract_u64(expr: &SExpr) -> Option<u64> {
    match expr {
        SExpr::Atom(s) => s.parse().ok(),
        SExpr::List(_) => None,
    }
}

fn extract_string(expr: &SExpr) -> String {
    match expr {
        SExpr::Atom(s) => {
            // Remove surrounding quotes if present
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s[1..s.len() - 1].to_string()
            } else {
                s.clone()
            }
        }
        SExpr::List(_) => String::new(),
    }
}

fn builtin_superblock(vm: &Vm, _args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    let inspector = get_inspector(vm).ok_or_else(|| {
        lispdown::SError::new("superblock").with_message("No inspector available")
    })?;
    let guard = inspector.lock().unwrap();
    match &guard.superblock {
        Some(sb) => Ok(sb.to_sexpr()),
        None => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom("no valid superblock".to_string()),
        ])),
    }
}

fn builtin_block(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("block").with_message("Usage: (block <address>)"));
    }
    let addr = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("block").with_message("Invalid block address"))?;

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("block").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    match guard.read_block(addr) {
        Ok(data) => {
            let mut items = vec![SExpr::Atom("block".to_string())];
            items.push(SExpr::List(vec![
                SExpr::Atom("address".to_string()),
                SExpr::Atom(addr.to_string()),
            ]));

            // Show first 256 bytes as hex lines
            let mut hex_lines = vec![SExpr::Atom("preview".to_string())];
            for row in 0..16 {
                let start = row * 16;
                let hex: String = data[start..start + 16]
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<Vec<_>>()
                    .join(" ");
                hex_lines.push(SExpr::List(vec![
                    SExpr::Atom(format!("{:04x}", start)),
                    SExpr::Atom(format!("\"{}\"", hex)),
                ]));
            }
            items.push(SExpr::List(hex_lines));
            items.push(SExpr::List(vec![
                SExpr::Atom("remaining".to_string()),
                SExpr::Atom((BLOCK_SIZE - 256).to_string()),
            ]));

            Ok(SExpr::List(items))
        }
        Err(e) => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom(format!("failed to read block {}: {}", addr, e)),
        ])),
    }
}

fn builtin_inode(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("inode").with_message("Usage: (inode <number>)"));
    }
    let ino = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("inode").with_message("Invalid inode number"))?;

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("inode").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    match guard.read_inode(ino) {
        Ok(Some(inode)) => Ok(inode.to_sexpr()),
        Ok(None) => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom(format!("inode {} not found (run (scan) first)", ino)),
        ])),
        Err(e) => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom(format!("failed to read inode: {}", e)),
        ])),
    }
}

fn builtin_dir(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("dir").with_message("Usage: (dir <inode-number>)"));
    }
    let ino = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("dir").with_message("Invalid inode number"))?;

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("dir").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    guard
        .dump_directory(ino)
        .map_err(|e| lispdown::SError::new("dir").with_message(&format!("IO error: {}", e)))
}

fn builtin_path(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("path").with_message("Usage: (path <path-string>)"));
    }
    let path = extract_string(&args[0]);

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("path").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    match guard.resolve_path(&path) {
        Ok(Some(ino)) => {
            // Also return the inode info
            match guard.read_inode(ino) {
                Ok(Some(inode)) => {
                    let mut items = vec![
                        SExpr::Atom("path-result".to_string()),
                        SExpr::List(vec![
                            SExpr::Atom("path".to_string()),
                            SExpr::Atom(format!("\"{}\"", path)),
                        ]),
                    ];
                    items.push(inode.to_sexpr());
                    Ok(SExpr::List(items))
                }
                _ => Ok(SExpr::List(vec![
                    SExpr::Atom("path-result".to_string()),
                    SExpr::List(vec![
                        SExpr::Atom("path".to_string()),
                        SExpr::Atom(format!("\"{}\"", path)),
                    ]),
                    SExpr::List(vec![
                        SExpr::Atom("ino".to_string()),
                        SExpr::Atom(ino.to_string()),
                    ]),
                ])),
            }
        }
        Ok(None) => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom(format!("path not found: {}", path)),
        ])),
        Err(e) => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom(format!("IO error: {}", e)),
        ])),
    }
}

fn builtin_blocks(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("blocks").with_message("Usage: (blocks <inode-number>)"));
    }
    let ino = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("blocks").with_message("Invalid inode number"))?;

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("blocks").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    guard
        .list_blocks(ino)
        .map_err(|e| lispdown::SError::new("blocks").with_message(&format!("IO error: {}", e)))
}

fn builtin_hexdump(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("hexdump")
            .with_message("Usage: (hexdump <block> [offset] [len])"));
    }
    let block = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("hexdump").with_message("Invalid block address"))?;
    let offset = args.get(1).and_then(extract_u64).unwrap_or(0) as usize;
    let len = args.get(2).and_then(extract_u64).unwrap_or(256) as usize;

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("hexdump").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    guard
        .hexdump_block(block, offset, len)
        .map_err(|e| lispdown::SError::new("hexdump").with_message(&format!("IO error: {}", e)))
}

fn builtin_block_type(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(
            lispdown::SError::new("block-type").with_message("Usage: (block-type <address>)")
        );
    }
    let addr = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("block-type").with_message("Invalid block address"))?;

    let inspector = get_inspector(vm).ok_or_else(|| {
        lispdown::SError::new("block-type").with_message("No inspector available")
    })?;
    let mut guard = inspector.lock().unwrap();

    guard
        .identify_block(addr)
        .map_err(|e| lispdown::SError::new("block-type").with_message(&format!("IO error: {}", e)))
}

fn builtin_owner(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    if args.is_empty() {
        return Err(lispdown::SError::new("owner").with_message("Usage: (owner <block-address>)"));
    }
    let addr = extract_u64(&args[0])
        .ok_or_else(|| lispdown::SError::new("owner").with_message("Invalid block address"))?;

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("owner").with_message("No inspector available"))?;
    let guard = inspector.lock().unwrap();

    if let Some((ino, idx, typ)) = guard.block_owners.get(&addr) {
        Ok(SExpr::List(vec![
            SExpr::Atom("owner".to_string()),
            SExpr::List(vec![
                SExpr::Atom("block".to_string()),
                SExpr::Atom(addr.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("ino".to_string()),
                SExpr::Atom(ino.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("index".to_string()),
                SExpr::Atom(idx.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("type".to_string()),
                SExpr::Atom(typ.clone()),
            ]),
        ]))
    } else if guard.scanned {
        Ok(SExpr::List(vec![
            SExpr::Atom("owner".to_string()),
            SExpr::List(vec![
                SExpr::Atom("block".to_string()),
                SExpr::Atom(addr.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("status".to_string()),
                SExpr::Atom("free-or-garbage".to_string()),
            ]),
        ]))
    } else {
        Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom("run (scan) first to build block index".to_string()),
        ]))
    }
}

fn builtin_log_status(vm: &Vm, _args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    let inspector = get_inspector(vm).ok_or_else(|| {
        lispdown::SError::new("log-status").with_message("No inspector available")
    })?;
    let guard = inspector.lock().unwrap();
    Ok(guard.log_status())
}

fn builtin_scan(vm: &Vm, _args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("scan").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    match guard.scan() {
        Ok((inode_count, block_count)) => Ok(SExpr::List(vec![
            SExpr::Atom("scan-result".to_string()),
            SExpr::List(vec![
                SExpr::Atom("inodes".to_string()),
                SExpr::Atom(inode_count.to_string()),
            ]),
            SExpr::List(vec![
                SExpr::Atom("blocks-indexed".to_string()),
                SExpr::Atom(block_count.to_string()),
            ]),
        ])),
        Err(e) => Ok(SExpr::List(vec![
            SExpr::Atom("error".to_string()),
            SExpr::Atom(format!("scan failed: {}", e)),
        ])),
    }
}

fn builtin_check(vm: &Vm, _args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("check").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    guard
        .run_checks()
        .map_err(|e| lispdown::SError::new("check").with_message(&format!("IO error: {}", e)))
}

fn builtin_tree(vm: &Vm, args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    let path = if args.is_empty() {
        "/".to_string()
    } else {
        extract_string(&args[0])
    };

    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("tree").with_message("No inspector available"))?;
    let mut guard = inspector.lock().unwrap();

    guard
        .show_tree(&path)
        .map_err(|e| lispdown::SError::new("tree").with_message(&format!("IO error: {}", e)))
}

fn builtin_help(_vm: &Vm, _args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    Ok(SExpr::List(vec![
        SExpr::Atom("help".to_string()),
        SExpr::List(vec![
            SExpr::Atom("superblock".to_string()),
            SExpr::Atom("\"Display superblock information\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("log-status".to_string()),
            SExpr::Atom("\"Show log head/tail status with visualization\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(block N)".to_string()),
            SExpr::Atom("\"Read and display raw block at address N\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(hexdump N [off] [len])".to_string()),
            SExpr::Atom("\"Hexdump block N (default: offset=0, len=256)\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(block-type N)".to_string()),
            SExpr::Atom("\"Identify what type of block N is\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(owner N)".to_string()),
            SExpr::Atom("\"Find which inode owns block N\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(inode N)".to_string()),
            SExpr::Atom("\"Read and display inode N\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(blocks N)".to_string()),
            SExpr::Atom("\"List all blocks owned by inode N\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(dir N)".to_string()),
            SExpr::Atom("\"Dump directory contents for inode N\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(path \"/a/b/c\")".to_string()),
            SExpr::Atom("\"Resolve path and show inode info\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(tree [path])".to_string()),
            SExpr::Atom("\"Show directory tree (default: /)\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(scan)".to_string()),
            SExpr::Atom("\"Scan inode map and build block index\"".to_string()),
        ]),
        SExpr::List(vec![
            SExpr::Atom("(check)".to_string()),
            SExpr::Atom("\"Run filesystem integrity checks\"".to_string()),
        ]),
    ]))
}

fn builtin_inodes(vm: &Vm, _args: &[SExpr]) -> Result<SExpr, lispdown::SError> {
    let inspector = get_inspector(vm)
        .ok_or_else(|| lispdown::SError::new("inodes").with_message("No inspector available"))?;
    let guard = inspector.lock().unwrap();

    let mut items = vec![SExpr::Atom("inodes".to_string())];
    for (&ino, &block) in &guard.inode_map {
        items.push(SExpr::List(vec![
            SExpr::Atom(ino.to_string()),
            SExpr::Atom(block.to_string()),
        ]));
    }
    Ok(SExpr::List(items))
}

////////////////////////////////////////////// LispHelper ////////////////////////////////////////////

/// Helper for rustyline with parenthesis validation.
#[derive(Default)]
struct LispHelper {
    function_names: Vec<String>,
}

impl LispHelper {
    fn new(function_names: Vec<String>) -> Self {
        Self { function_names }
    }
}

impl Validator for LispHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> Result<ValidationResult, ReadlineError> {
        let input = ctx.input();
        if is_balanced(input) {
            Ok(ValidationResult::Valid(None))
        } else {
            Ok(ValidationResult::Incomplete)
        }
    }
}

impl Completer for LispHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        let start = line[..pos]
            .rfind(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prefix = &line[start..pos];

        if prefix.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let matches: Vec<Pair> = self
            .function_names
            .iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| Pair {
                display: name.clone(),
                replacement: name.clone(),
            })
            .collect();

        Ok((start, matches))
    }
}

impl Hinter for LispHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for LispHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        Cow::Borrowed(line)
    }
    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool {
        false
    }
}

impl Helper for LispHelper {}

/// Checks if parentheses are balanced.
fn is_balanced(input: &str) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for c in input.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match c {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth -= 1,
            _ => {}
        }
    }

    depth <= 0 && !in_string
}

/////////////////////////////////////////////// Main /////////////////////////////////////////////////

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <disk-image>", args[0]);
        eprintln!();
        eprintln!("Interactive Lisp REPL for debugging eudaemonfs.");
        eprintln!("Opens a disk image file and provides s-expression commands");
        eprintln!("to inspect the filesystem structure at a low level.");
        eprintln!();
        eprintln!("Example: {} /path/to/disk.img", args[0]);
        std::process::exit(1);
    }

    let disk_path = &args[1];

    let inspector = match Inspector::new(disk_path) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Error opening {}: {}", disk_path, e);
            std::process::exit(1);
        }
    };

    println!("inspect-fs: eudaemonfs debugger (Lisp REPL)");
    println!("Opened: {}", disk_path);

    if let Some(sb) = &inspector.superblock {
        if sb.is_valid() {
            println!(
                "Valid eudaemonfs filesystem: {} blocks, log {}-{}",
                sb.total_blocks, sb.log_start, sb.log_end
            );
        } else {
            println!("WARNING: Invalid superblock magic!");
            println!("  Expected: 0x{:016x}", MAGIC);
            println!("  Got:      0x{:016x}", sb.magic);
        }
    } else {
        println!("WARNING: Could not parse superblock!");
    }

    println!();
    println!("Type (help) for commands, Ctrl-D to exit");
    println!();

    // Create VM and register builtins
    let mut vm = Vm::new();
    vm.register_builtins();

    // Register inspector builtins
    vm.def_fn("superblock", builtin_superblock);
    vm.def_fn("block", builtin_block);
    vm.def_fn("inode", builtin_inode);
    vm.def_fn("dir", builtin_dir);
    vm.def_fn("path", builtin_path);
    vm.def_fn("blocks", builtin_blocks);
    vm.def_fn("hexdump", builtin_hexdump);
    vm.def_fn("block-type", builtin_block_type);
    vm.def_fn("owner", builtin_owner);
    vm.def_fn("log-status", builtin_log_status);
    vm.def_fn("scan", builtin_scan);
    vm.def_fn("check", builtin_check);
    vm.def_fn("tree", builtin_tree);
    vm.def_fn("help", builtin_help);
    vm.def_fn("inodes", builtin_inodes);

    // Store inspector in VM
    let shared_inspector: SharedInspector = Arc::new(Mutex::new(inspector));
    vm.set_user_data(shared_inspector.clone());

    // Auto-scan on startup
    println!("Auto-scanning filesystem...");
    {
        let mut guard = shared_inspector.lock().unwrap();
        match guard.scan() {
            Ok((inodes, blocks)) => {
                println!("Loaded {} inodes, indexed {} blocks", inodes, blocks);
            }
            Err(e) => {
                println!("Warning: scan failed: {}", e);
            }
        }
    }
    println!();

    // Setup rustyline
    let function_names = vec![
        "superblock",
        "log-status",
        "block",
        "hexdump",
        "block-type",
        "owner",
        "inode",
        "inodes",
        "blocks",
        "dir",
        "path",
        "tree",
        "scan",
        "check",
        "help",
        "quote",
        "if",
        "let",
        "begin",
        "list",
        "first",
        "rest",
        "cons",
        "append",
        "length",
        "nth",
        "map",
        "filter",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let helper = LispHelper::new(function_names);
    let mut rl: Editor<LispHelper, rustyline::history::DefaultHistory> = match Editor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to initialize readline: {}", e);
            std::process::exit(1);
        }
    };

    rl.set_helper(Some(helper));
    rl.set_edit_mode(EditMode::Vi);

    loop {
        match rl.readline("λ> ") {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(&line);

                // Parse and evaluate
                let mut parser = Parser::new(input);
                match parser.parse() {
                    Ok(expr) => match vm.eval(&expr) {
                        Ok(result) => {
                            println!("{}", result);
                        }
                        Err(e) => {
                            println!("Error: {}", e);
                        }
                    },
                    Err(e) => {
                        println!("Parse error: {}", e);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("^D");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    println!("Goodbye!");
}
