//! SynFS: Synthetic filesystem tests using Guacamole for deterministic random data.
//!
//! This test uses Guacamole's linearly-seekable property to generate random file operations
//! where the writes can be verified by regenerating the expected content from the seed.
//!
//! Key properties:
//! - Given the same seed, Guacamole produces the same output.
//! - Seeds 1 unit apart generate streams offset by 64 bytes.
//! - This allows writes at different offsets to compose into verifiable content.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use guacamole::Guacamole;
use guacamole::combinators::CHAR_SET_LOWER;
use guacamole::combinators::any;
use guacamole::combinators::coin;
use guacamole::combinators::range_to;
use guacamole::combinators::string;
use guacamole::combinators::to_charset;
use guacamole::combinators::uniform;

use synfs::DeviceId;
use synfs::Error;
use synfs::Lfs;

/// Block size in bytes (must match LFS).
const BLOCK_SIZE: usize = 4096;

fn zero_time() -> i64 {
    0
}

/// Number of blocks in the test filesystem.
const TEST_FS_BLOCKS: usize = 512;

/// Bytes per Guacamole seed increment.
const GUAC_BYTES_PER_SEED: u64 = 64;

/// Maximum file size we'll generate.
const MAX_FILE_SIZE: u64 = 32 * 1024;

/// Maximum path depth.
const MAX_DEPTH: usize = 3;

///////////////////////////////////////////// SynFile ///////////////////////////////////////////////

/// A synthetic file with a base seed for content generation.
///
/// The file tracks which regions have been written. Unwritten regions
/// contain zeros (sparse file semantics).
#[derive(Debug, Clone)]
struct SynFile {
    /// Base seed for this file's content.
    base_seed: u64,
    /// Current size of the file.
    size: u64,
    /// Written regions as (start, end) pairs.
    written_regions: Vec<(u64, u64)>,
}

impl SynFile {
    fn new(base_seed: u64) -> Self {
        Self {
            base_seed,
            size: 0,
            written_regions: Vec::new(),
        }
    }

    /// Records a write to the file.
    fn record_write(&mut self, offset: u64, len: usize) {
        let end = offset + len as u64;
        self.written_regions.push((offset, end));
        // Merge overlapping regions for efficiency
        self.written_regions.sort_by_key(|r| r.0);
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for region in &self.written_regions {
            if let Some(last) = merged.last_mut() {
                if region.0 <= last.1 {
                    last.1 = last.1.max(region.1);
                } else {
                    merged.push(*region);
                }
            } else {
                merged.push(*region);
            }
        }
        self.written_regions = merged;
    }

    /// Generates content using guacamole for the given seed.
    fn generate_guac_content(&self, offset: u64, len: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(len);
        let mut pos = offset;
        let end = offset + len as u64;

        while pos < end {
            // Calculate which seed to use for this position
            let seed_offset = pos / GUAC_BYTES_PER_SEED;
            let byte_offset = (pos % GUAC_BYTES_PER_SEED) as usize;
            let seed = self.base_seed + seed_offset;

            // Generate 64 bytes from this seed
            let mut guac = Guacamole::new(seed);
            let mut buf = [0u8; GUAC_BYTES_PER_SEED as usize];
            for b in &mut buf {
                *b = any(&mut guac);
            }

            // Copy the relevant portion
            let bytes_from_seed =
                (GUAC_BYTES_PER_SEED as usize - byte_offset).min((end - pos) as usize);
            result.extend_from_slice(&buf[byte_offset..byte_offset + bytes_from_seed]);
            pos += bytes_from_seed as u64;
        }

        result
    }

    /// Generates the expected content for a range of the file.
    /// Unwritten regions are zeros, written regions have guacamole content.
    fn generate_content(&self, offset: u64, len: usize) -> Vec<u8> {
        let mut result = vec![0u8; len];
        let end = offset + len as u64;

        for &(start, region_end) in &self.written_regions {
            // Check if this region overlaps with our range
            if region_end <= offset || start >= end {
                continue;
            }

            // Calculate overlap
            let overlap_start = start.max(offset);
            let overlap_end = region_end.min(end);

            // Generate content for the overlap
            let guac_content =
                self.generate_guac_content(overlap_start, (overlap_end - overlap_start) as usize);

            // Copy into result
            let result_start = (overlap_start - offset) as usize;
            let result_end = (overlap_end - offset) as usize;
            result[result_start..result_end].copy_from_slice(&guac_content);
        }

        result
    }

    /// Generates data to write at a given offset.
    fn generate_write_data(&self, offset: u64, len: usize) -> Vec<u8> {
        self.generate_guac_content(offset, len)
    }
}

///////////////////////////////////////////// SynFs /////////////////////////////////////////////////

/// A synthetic filesystem that tracks files and their expected content.
struct SynFs {
    /// Maps path to file state.
    files: BTreeMap<String, SynFile>,
    /// Set of directories that exist.
    dirs: BTreeSet<String>,
    /// Next seed to use for new files.
    next_seed: u64,
    /// Seed increment per file to ensure distinct content.
    seed_stride: u64,
}

impl SynFs {
    fn new() -> Self {
        let mut dirs = BTreeSet::new();
        dirs.insert("/".to_string());
        Self {
            files: BTreeMap::new(),
            dirs,
            next_seed: 0,
            // Stride large enough to avoid overlap between files
            // MAX_FILE_SIZE / 64 = max seeds per file
            seed_stride: MAX_FILE_SIZE / GUAC_BYTES_PER_SEED + 1,
        }
    }

    /// Creates all parent directories for a path.
    fn mkdir_p(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            current.push('/');
            current.push_str(part);
            self.dirs.insert(current.clone());
        }
    }

    /// Creates a file at the given path.
    fn create_file(&mut self, path: &str) -> &mut SynFile {
        self.mkdir_p(path);
        let seed = self.next_seed;
        self.next_seed += self.seed_stride;
        self.files
            .entry(path.to_string())
            .or_insert_with(|| SynFile::new(seed))
    }

    /// Removes a file and cleans up empty parent directories.
    fn remove_file(&mut self, path: &str) -> bool {
        if self.files.remove(path).is_some() {
            // Clean up empty directories
            let mut current = path.to_string();
            while let Some(pos) = current.rfind('/') {
                if pos == 0 {
                    break;
                }
                current.truncate(pos);
                // Check if directory is empty
                let prefix = format!("{}/", current);
                let has_files = self.files.keys().any(|k| k.starts_with(&prefix));
                let has_subdirs = self
                    .dirs
                    .iter()
                    .any(|d| d != &current && d.starts_with(&prefix));
                if !has_files && !has_subdirs {
                    self.dirs.remove(&current);
                } else {
                    break;
                }
            }
            true
        } else {
            false
        }
    }

    /// Checks if a file exists.
    fn file_exists(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// Gets a mutable reference to a file.
    fn get_file_mut(&mut self, path: &str) -> Option<&mut SynFile> {
        self.files.get_mut(path)
    }

    /// Gets a reference to a file.
    fn get_file(&self, path: &str) -> Option<&SynFile> {
        self.files.get(path)
    }
}

///////////////////////////////////////// Path Generation ///////////////////////////////////////////

/// Generates a random path like `/a/b/c.txt`.
fn generate_path(guac: &mut Guacamole) -> String {
    let mut gen_name = string(|g| uniform(1usize, 4usize)(g), to_charset(CHAR_SET_LOWER));
    let depth: usize = uniform(1usize, MAX_DEPTH + 1)(guac);

    let mut path = String::new();
    for i in 0..depth {
        path.push('/');
        path.push_str(&gen_name(guac));
        if i == depth - 1 {
            path.push_str(".txt");
        }
    }
    path
}

///////////////////////////////////////// Test Operations ///////////////////////////////////////////

/// Operations that can be performed on the filesystem.
#[derive(Debug, Clone)]
enum Operation {
    /// Create a file and write data to it.
    CreateAndWrite {
        path: String,
        offset: u64,
        len: usize,
    },
    /// Write more data to an existing file.
    Write {
        path: String,
        offset: u64,
        len: usize,
    },
    /// Remove a file and clean up empty directories.
    Remove { path: String },
    /// Verify a file's content matches expected.
    Verify { path: String },
}

/// Generates a sequence of operations.
fn generate_operations(guac: &mut Guacamole, count: usize) -> Vec<Operation> {
    let mut ops = Vec::with_capacity(count);
    let mut known_paths: Vec<String> = Vec::new();

    for _ in 0..count {
        // Coin flip: create/write vs remove
        let do_create = coin()(guac) || known_paths.is_empty();

        if do_create {
            let path = generate_path(guac);
            let offset: u64 = uniform(0u64, MAX_FILE_SIZE / 2)(guac);
            let max_len = (MAX_FILE_SIZE - offset) as usize;
            let len: usize = uniform(1usize, max_len.clamp(2, 4096))(guac);

            if known_paths.contains(&path) {
                ops.push(Operation::Write {
                    path: path.clone(),
                    offset,
                    len,
                });
            } else {
                ops.push(Operation::CreateAndWrite {
                    path: path.clone(),
                    offset,
                    len,
                });
                known_paths.push(path);
            }
        } else {
            // Remove a random file
            let idx: usize = range_to(known_paths.len())(guac);
            let path = known_paths.remove(idx);
            ops.push(Operation::Remove { path });
        }
    }

    // Add verify operations for remaining files
    for path in &known_paths {
        ops.push(Operation::Verify { path: path.clone() });
    }

    ops
}

///////////////////////////////////////// Test Execution ////////////////////////////////////////////

/// Executes operations on both the reference SynFs and the real LFS.
/// Tracks which files were actually written successfully to handle NoSpace errors.
fn execute_operations(_seed: u64, ops: &[Operation]) {
    let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
    let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("Failed to create LFS");
    let mut synfs = SynFs::new();
    // Track files that were successfully written to LFS
    let mut written_files: BTreeSet<String> = BTreeSet::new();

    for op in ops {
        match op {
            Operation::CreateAndWrite { path, offset, len } => {
                // Create file in synfs
                let syn_file = synfs.create_file(path);

                // Generate the data to write
                let write_data = syn_file.generate_write_data(*offset, *len);

                // Create file in LFS and write
                // LFS uses flat namespace, so we use the path as filename
                let lfs_name = path_to_lfs_name(path);
                let fd = match lfs.open_file(&lfs_name) {
                    Ok(fd) => fd,
                    Err(Error::NoSpace) => {
                        // Failed to create, remove from synfs tracking
                        synfs.remove_file(path);
                        continue;
                    }
                    Err(e) => panic!("Failed to open file {}: {:?}", path, e),
                };

                if *offset > 0 {
                    lfs.seek(fd, *offset).expect("Failed to seek");
                }

                match lfs.write(fd, &write_data) {
                    Ok(_) => {
                        // Update synfs state only on successful write
                        if let Some(f) = synfs.get_file_mut(path) {
                            f.record_write(*offset, *len);
                            let new_size = offset + *len as u64;
                            if new_size > f.size {
                                f.size = new_size;
                            }
                        }
                        written_files.insert(path.clone());
                    }
                    Err(Error::NoSpace) | Err(Error::FileTooLarge) => {
                        // Write failed, remove from synfs tracking
                        synfs.remove_file(path);
                        lfs.close(fd).expect("Failed to close");
                        continue;
                    }
                    Err(e) => panic!("Failed to write: {:?}", e),
                }

                lfs.close(fd).expect("Failed to close");
            }

            Operation::Write { path, offset, len } => {
                // Skip if file doesn't exist in synfs or wasn't written to LFS
                if !synfs.file_exists(path) || !written_files.contains(path) {
                    continue;
                }

                // Get existing file from synfs
                let syn_file = synfs.get_file_mut(path).unwrap();

                // Generate the data to write
                let write_data = syn_file.generate_write_data(*offset, *len);

                // Write to LFS
                let lfs_name = path_to_lfs_name(path);
                let fd = match lfs.open_file(&lfs_name) {
                    Ok(fd) => fd,
                    Err(Error::NoSpace) => continue,
                    Err(e) => panic!("Failed to open file {}: {:?}", path, e),
                };

                lfs.seek(fd, *offset).expect("Failed to seek");

                match lfs.write(fd, &write_data) {
                    Ok(_) => {
                        // Update synfs state only on successful write
                        if let Some(f) = synfs.get_file_mut(path) {
                            f.record_write(*offset, *len);
                            let new_size = offset + *len as u64;
                            if new_size > f.size {
                                f.size = new_size;
                            }
                        }
                    }
                    Err(Error::NoSpace) | Err(Error::FileTooLarge) => {
                        lfs.close(fd).expect("Failed to close");
                        continue;
                    }
                    Err(e) => panic!("Failed to write: {:?}", e),
                }

                lfs.close(fd).expect("Failed to close");
            }

            Operation::Remove { path } => {
                // Remove from tracking
                written_files.remove(path);

                // Remove from synfs
                synfs.remove_file(path);

                // Remove from LFS
                let lfs_name = path_to_lfs_name(path);
                let _ = lfs.remove(&lfs_name); // May fail if file doesn't exist
            }

            Operation::Verify { path } => {
                // Skip if file wasn't successfully written
                if !written_files.contains(path) {
                    continue;
                }

                // Get expected content from synfs
                let syn_file = match synfs.get_file(path) {
                    Some(f) => f,
                    None => continue, // File was removed
                };

                let expected = syn_file.generate_content(0, syn_file.size as usize);

                // Read from LFS
                let lfs_name = path_to_lfs_name(path);
                let fd = lfs
                    .open_file(&lfs_name)
                    .expect("Failed to open file for verify");
                lfs.seek(fd, 0).expect("Failed to seek");

                let mut actual = vec![0u8; syn_file.size as usize];
                let n = lfs.read(fd, &mut actual).expect("Failed to read");
                lfs.close(fd).expect("Failed to close");

                assert_eq!(n, expected.len(), "Size mismatch for {}", path);
                assert_eq!(actual, expected, "Content mismatch for {}", path);
            }
        }
    }

    // Final verification: check all written files in synfs exist in LFS with correct content
    for path in &written_files {
        let syn_file = match synfs.get_file(path) {
            Some(f) => f,
            None => continue, // File was removed
        };

        let expected = syn_file.generate_content(0, syn_file.size as usize);
        let lfs_name = path_to_lfs_name(path);

        let fd = lfs
            .open_file(&lfs_name)
            .expect("Failed to open file for final verify");
        lfs.seek(fd, 0).expect("Failed to seek");

        let mut actual = vec![0u8; syn_file.size as usize];
        let n = lfs.read(fd, &mut actual).expect("Failed to read");
        lfs.close(fd).expect("Failed to close");

        assert_eq!(n, expected.len(), "Final size mismatch for {}", path);
        assert_eq!(actual, expected, "Final content mismatch for {}", path);
    }
}

/// Converts a hierarchical path to a flat LFS filename.
fn path_to_lfs_name(path: &str) -> String {
    // Replace / with _ to create flat namespace
    // Skip leading /
    path.trim_start_matches('/').replace('/', "_")
}

////////////////////////////////////////////// Tests ////////////////////////////////////////////////

#[test]
fn synfs_basic() {
    let mut guac = Guacamole::new(12345);
    let ops = generate_operations(&mut guac, 50);
    println!("Generated {} operations", ops.len());
    for op in &ops {
        println!("  {:?}", op);
    }
    execute_operations(12345, &ops);
}

#[test]
fn synfs_many_operations() {
    let mut guac = Guacamole::new(67890);
    let ops = generate_operations(&mut guac, 200);
    execute_operations(67890, &ops);
}

#[test]
fn synfs_seed_sweep() {
    for seed in 0..100 {
        let mut guac = Guacamole::new(seed);
        let ops = generate_operations(&mut guac, 50);
        execute_operations(seed, &ops);
    }
}

#[test]
fn guacamole_linear_seekable_property() {
    // Verify the linear-seekable property that the test relies on
    let mut stream1 = Guacamole::new(0);
    let mut stream2 = Guacamole::new(1);

    let mut buffer1 = [0u8; 128];
    for b in &mut buffer1[..64] {
        *b = any(&mut stream1);
    }
    for b in &mut buffer1[64..] {
        *b = any(&mut stream2);
    }

    let mut stream3 = Guacamole::new(0);
    let mut buffer2 = [0u8; 128];
    for b in &mut buffer2[..64] {
        *b = any(&mut stream3);
    }
    // stream3 should now be at nonce 1, same as stream2 started
    for b in &mut buffer2[64..] {
        *b = any(&mut stream3);
    }

    assert_eq!(buffer1, buffer2, "Linear seekable property violated");
    println!("Linear seekable property verified");
}

#[test]
fn synfile_content_generation() {
    // Test that SynFile generates consistent content
    let file = SynFile::new(1000);

    // Generate content at offset 0
    let content1 = file.generate_content(0, 100);

    // Generate content at offset 50
    let content2 = file.generate_content(50, 50);

    // The last 50 bytes of content1 should equal content2
    assert_eq!(
        &content1[50..],
        &content2[..],
        "Content generation inconsistent"
    );
    println!("SynFile content generation verified");
}

#[test]
fn synfile_across_seed_boundaries() {
    // Test content generation that spans multiple seed boundaries (64-byte chunks)
    let file = SynFile::new(0);

    // Generate 200 bytes starting at offset 0
    let full = file.generate_content(0, 200);

    // Generate in chunks that span boundaries
    let chunk1 = file.generate_content(0, 70); // Spans seeds 0-1
    let chunk2 = file.generate_content(70, 60); // Spans seeds 1-2
    let chunk3 = file.generate_content(130, 70); // Spans seeds 2-3

    assert_eq!(&full[0..70], &chunk1[..], "Chunk 1 mismatch");
    assert_eq!(&full[70..130], &chunk2[..], "Chunk 2 mismatch");
    assert_eq!(&full[130..200], &chunk3[..], "Chunk 3 mismatch");
    println!("Cross-boundary content generation verified");
}

#[test]
fn synfs_persist_and_restore() {
    let mut guac = Guacamole::new(99999);
    let ops = generate_operations(&mut guac, 30);

    // Build expected state
    let data = vec![0u8; TEST_FS_BLOCKS * BLOCK_SIZE];
    let mut lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time).expect("Failed to create LFS");
    let mut synfs = SynFs::new();
    let mut written_files: BTreeSet<String> = BTreeSet::new();

    // Execute operations (without verify ops)
    for op in &ops {
        match op {
            Operation::CreateAndWrite { path, offset, len } => {
                let syn_file = synfs.create_file(path);
                let write_data = syn_file.generate_write_data(*offset, *len);

                let lfs_name = path_to_lfs_name(path);
                if let Ok(fd) = lfs.open_file(&lfs_name) {
                    if *offset > 0 {
                        let _ = lfs.seek(fd, *offset);
                    }
                    if lfs.write(fd, &write_data).is_ok() {
                        if let Some(f) = synfs.get_file_mut(path) {
                            f.record_write(*offset, *len);
                            let new_size = offset + *len as u64;
                            if new_size > f.size {
                                f.size = new_size;
                            }
                        }
                        written_files.insert(path.clone());
                    } else {
                        synfs.remove_file(path);
                    }
                    let _ = lfs.close(fd);
                } else {
                    synfs.remove_file(path);
                }
            }
            Operation::Write { path, offset, len } => {
                if !written_files.contains(path) {
                    continue;
                }
                if let Some(syn_file) = synfs.get_file_mut(path) {
                    let write_data = syn_file.generate_write_data(*offset, *len);

                    let lfs_name = path_to_lfs_name(path);
                    if let Ok(fd) = lfs.open_file(&lfs_name) {
                        let _ = lfs.seek(fd, *offset);
                        if lfs.write(fd, &write_data).is_ok()
                            && let Some(f) = synfs.get_file_mut(path)
                        {
                            f.record_write(*offset, *len);
                            let new_size = offset + *len as u64;
                            if new_size > f.size {
                                f.size = new_size;
                            }
                        }
                        let _ = lfs.close(fd);
                    }
                }
            }
            Operation::Remove { path } => {
                written_files.remove(path);
                synfs.remove_file(path);
                let lfs_name = path_to_lfs_name(path);
                let _ = lfs.remove(&lfs_name);
            }
            Operation::Verify { .. } => {}
        }
    }

    // Persist and restore
    let raw_data = lfs.into_inner();
    let mut lfs2 =
        Lfs::open_vec(raw_data, DeviceId::new(1), zero_time).expect("Failed to restore LFS");

    // Verify all written files after restore
    for path in &written_files {
        let syn_file = match synfs.get_file(path) {
            Some(f) => f,
            None => continue,
        };

        let expected = syn_file.generate_content(0, syn_file.size as usize);
        let lfs_name = path_to_lfs_name(path);

        let fd = lfs2
            .open_file(&lfs_name)
            .expect("Failed to open after restore");
        lfs2.seek(fd, 0).expect("Failed to seek");

        let mut actual = vec![0u8; syn_file.size as usize];
        let n = lfs2.read(fd, &mut actual).expect("Failed to read");
        lfs2.close(fd).expect("Failed to close");

        assert_eq!(n, expected.len(), "Restore size mismatch for {}", path);
        assert_eq!(actual, expected, "Restore content mismatch for {}", path);
    }
    println!("Persist and restore verified");
}
