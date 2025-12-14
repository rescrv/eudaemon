//! Stress test for SynFS using Guacamole for deterministic random operations.
//!
//! This program generates random file operations to stress test the filesystem:
//! - Branch 1: Create a file at a random path and write data at a random offset
//! - Branch 2: Remove a random file and clean up empty parent directories
//!
//! The test uses Guacamole's linearly-seekable property to generate verifiable content.
//! Given the same seed, the generated content will be identical, allowing verification
//! of file contents after writes.
//!
//! The test runs until the specified amount of data has been written. When NoSpace is
//! encountered, the test prefers branch 2 (removal) to free space.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use getopts::Options;
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
use synfs::MemoryBlockDevice;

/// Block size in bytes.
const BLOCK_SIZE: usize = 4096;

/// Bytes per Guacamole seed increment.
const GUAC_BYTES_PER_SEED: u64 = 64;

/// Maximum file size for a single write.
const MAX_WRITE_SIZE: usize = 64 * 1024;

/// Maximum path depth (e.g., 3 means /a/b/c.txt).
const MAX_DEPTH: usize = 3;

/// Seed stride between files to ensure distinct content.
const SEED_STRIDE: u64 = MAX_WRITE_SIZE as u64 / GUAC_BYTES_PER_SEED + 1024;

/// Usage threshold at which to run the cleaner proactively (75%).
const CLEANER_THRESHOLD_PERCENT: u64 = 75;

/// Usage threshold at which to block writes (80%).
const WRITE_BLOCK_THRESHOLD_PERCENT: u64 = 80;

/////////////////////////////////////////////// Config //////////////////////////////////////////////

/// Configuration for the stress test.
struct Config {
    /// Random seed for deterministic operation.
    seed: u64,
    /// Filesystem size in bytes.
    fs_size: u64,
    /// Total bytes to write before stopping.
    target_bytes: u64,
}

impl Config {
    fn from_args() -> Result<Self, String> {
        let args: Vec<String> = std::env::args().collect();
        let program = &args[0];

        let mut opts = Options::new();
        opts.optopt("s", "seed", "random seed (default: 12345)", "SEED");
        opts.optopt(
            "f",
            "fs-size",
            "filesystem size (default: 1G, supports K/M/G/T suffixes)",
            "SIZE",
        );
        opts.optopt(
            "t",
            "target",
            "total bytes to write (default: 1T, supports K/M/G/T suffixes)",
            "SIZE",
        );
        opts.optflag("h", "help", "print this help message");

        let matches = opts
            .parse(&args[1..])
            .map_err(|e| format!("Error parsing arguments: {}", e))?;

        if matches.opt_present("h") {
            let brief = format!("Usage: {} [options]", program);
            print!("{}", opts.usage(&brief));
            std::process::exit(0);
        }

        let seed = matches
            .opt_str("s")
            .map(|s| s.parse::<u64>())
            .transpose()
            .map_err(|e| format!("Invalid seed: {}", e))?
            .unwrap_or(12345);

        let fs_size = matches
            .opt_str("f")
            .map(|s| parse_size(&s))
            .transpose()?
            .unwrap_or(1024 * 1024 * 1024); // 1 GiB default

        let target_bytes = matches
            .opt_str("t")
            .map(|s| parse_size(&s))
            .transpose()?
            .unwrap_or(1024 * 1024 * 1024 * 1024); // 1 TiB default

        Ok(Self {
            seed,
            fs_size,
            target_bytes,
        })
    }
}

/// Parses a size string with optional K/M/G/T suffix.
fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty size string".to_string());
    }

    let (num_str, multiplier) = if let Some(prefix) = s.strip_suffix(['K', 'k']) {
        (prefix, 1024u64)
    } else if let Some(prefix) = s.strip_suffix(['M', 'm']) {
        (prefix, 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix(['G', 'g']) {
        (prefix, 1024 * 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix(['T', 't']) {
        (prefix, 1024 * 1024 * 1024 * 1024)
    } else {
        (s, 1)
    };

    let num: u64 = num_str
        .trim()
        .parse()
        .map_err(|e| format!("Invalid number '{}': {}", num_str, e))?;

    num.checked_mul(multiplier)
        .ok_or_else(|| format!("Size overflow: {} * {}", num, multiplier))
}

/// Formats a byte count with appropriate suffix.
fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 * 1024 {
        format!("{} TiB", bytes / (1024 * 1024 * 1024 * 1024))
    } else if bytes >= 1024 * 1024 * 1024 {
        format!("{} GiB", bytes / (1024 * 1024 * 1024))
    } else if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{} bytes", bytes)
    }
}

///////////////////////////////////////////// SynFile ///////////////////////////////////////////////

/// A synthetic file with a base seed for content generation.
struct SynFile {
    /// Base seed for this file's content.
    base_seed: u64,
    /// Current size of the file.
    size: u64,
    /// Written regions as (start, end) pairs (merged).
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

    /// Records a write to the file, merging overlapping regions.
    fn record_write(&mut self, offset: u64, len: usize) {
        let end = offset + len as u64;
        self.written_regions.push((offset, end));
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
        if end > self.size {
            self.size = end;
        }
    }

    /// Generates content using guacamole for the given offset and length.
    fn generate_guac_content(&self, offset: u64, len: usize) -> Vec<u8> {
        let mut result = Vec::with_capacity(len);
        let mut pos = offset;
        let end = offset + len as u64;

        while pos < end {
            let seed_offset = pos / GUAC_BYTES_PER_SEED;
            let byte_offset = (pos % GUAC_BYTES_PER_SEED) as usize;
            let seed = self.base_seed + seed_offset;

            let mut guac = Guacamole::new(seed);
            let mut buf = [0u8; GUAC_BYTES_PER_SEED as usize];
            for b in &mut buf {
                *b = any(&mut guac);
            }

            let bytes_from_seed =
                (GUAC_BYTES_PER_SEED as usize - byte_offset).min((end - pos) as usize);
            result.extend_from_slice(&buf[byte_offset..byte_offset + bytes_from_seed]);
            pos += bytes_from_seed as u64;
        }

        result
    }

    /// Generates the expected content for a range of the file.
    fn generate_content(&self, offset: u64, len: usize) -> Vec<u8> {
        let mut result = vec![0u8; len];
        let end = offset + len as u64;

        for &(start, region_end) in &self.written_regions {
            if region_end <= offset || start >= end {
                continue;
            }
            let overlap_start = start.max(offset);
            let overlap_end = region_end.min(end);
            let guac_content =
                self.generate_guac_content(overlap_start, (overlap_end - overlap_start) as usize);
            let result_start = (overlap_start - offset) as usize;
            let result_end = (overlap_end - offset) as usize;
            result[result_start..result_end].copy_from_slice(&guac_content);
        }

        result
    }
}

///////////////////////////////////////////// StressTest ////////////////////////////////////////////

/// State for the stress test.
struct StressTest {
    /// The filesystem being tested.
    lfs: Lfs<MemoryBlockDevice, fn() -> i64>,
    /// Maps path to file state for verification.
    files: BTreeMap<String, SynFile>,
    /// Set of directories that exist.
    dirs: BTreeSet<String>,
    /// Next seed to use for new files.
    next_seed: u64,
    /// Total bytes written so far.
    total_bytes_written: u64,
    /// RNG state.
    guac: Guacamole,
    /// Count of successful writes.
    write_count: u64,
    /// Count of successful removes.
    remove_count: u64,
    /// Last progress report bytes.
    last_progress_bytes: u64,
    /// Target bytes to write.
    target_bytes: u64,
    /// Filesystem size in blocks.
    fs_blocks: usize,
}

fn zero_time() -> i64 {
    0
}

impl StressTest {
    fn new(config: &Config) -> Self {
        let fs_blocks = (config.fs_size / BLOCK_SIZE as u64) as usize;
        let data = vec![0u8; fs_blocks * BLOCK_SIZE];
        let lfs = Lfs::from_vec(data, DeviceId::new(1), zero_time as fn() -> i64)
            .expect("Failed to create LFS");

        let mut dirs = BTreeSet::new();
        dirs.insert("/".to_string());

        Self {
            lfs,
            files: BTreeMap::new(),
            dirs,
            next_seed: 0,
            total_bytes_written: 0,
            guac: Guacamole::new(config.seed),
            write_count: 0,
            remove_count: 0,
            last_progress_bytes: 0,
            target_bytes: config.target_bytes,
            fs_blocks,
        }
    }

    /// Generates a random path like `/a/b/c.txt`.
    fn generate_path(&mut self) -> String {
        let mut gen_name = string(|g| uniform(1usize, 4usize)(g), to_charset(CHAR_SET_LOWER));
        let depth: usize = uniform(1usize, MAX_DEPTH + 1)(&mut self.guac);

        let mut path = String::new();
        for i in 0..depth {
            path.push('/');
            path.push_str(&gen_name(&mut self.guac));
            if i == depth - 1 {
                path.push_str(".txt");
            }
        }
        path
    }

    /// Creates all parent directories for a path in both tracking and LFS.
    fn mkdir_p(&mut self, path: &str) -> Result<(), Error> {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut current = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            current.push('/');
            current.push_str(part);
            if !self.dirs.contains(&current) {
                self.lfs.mkdir(&current)?;
                self.dirs.insert(current.clone());
            }
        }
        Ok(())
    }

    /// Selects a random existing file path, if any exist.
    fn select_random_file(&mut self) -> Option<String> {
        if self.files.is_empty() {
            return None;
        }
        let paths: Vec<&String> = self.files.keys().collect();
        let idx: usize = range_to(paths.len())(&mut self.guac);
        Some(paths[idx].clone())
    }

    /// Removes empty parent directories after file removal.
    fn cleanup_empty_dirs(&mut self, path: &str) {
        let mut current = path.to_string();
        while let Some(pos) = current.rfind('/') {
            if pos == 0 {
                break;
            }
            current.truncate(pos);
            let prefix = format!("{}/", current);
            let has_files = self.files.keys().any(|k| k.starts_with(&prefix));
            let has_subdirs = self
                .dirs
                .iter()
                .any(|d| d != &current && d.starts_with(&prefix));
            if !has_files && !has_subdirs {
                if self.lfs.rmdir(&current).is_ok() {
                    self.dirs.remove(&current);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Branch 1: Create a file and write data at a random offset.
    fn branch_create_and_write(&mut self) -> Result<usize, Error> {
        let path = self.generate_path();

        // Create parent directories
        self.mkdir_p(&path)?;

        // Get or create file tracking
        if !self.files.contains_key(&path) {
            let seed = self.next_seed;
            self.next_seed += SEED_STRIDE;
            self.files.insert(path.clone(), SynFile::new(seed));
        }

        // Generate random offset and length
        let offset: u64 = uniform(0u64, MAX_WRITE_SIZE as u64)(&mut self.guac);
        let max_len = MAX_WRITE_SIZE - (offset as usize).min(MAX_WRITE_SIZE);
        let len: usize = uniform(1usize, max_len.max(2))(&mut self.guac);

        // Generate content using the file's seed
        let syn_file = self.files.get(&path).unwrap();
        let write_data = syn_file.generate_guac_content(offset, len);

        // Open file in LFS (stripping leading slash for flat namespace compatibility)
        let lfs_path = path.trim_start_matches('/');
        let fd = self.lfs.open_file(lfs_path)?;

        if offset > 0 {
            self.lfs.seek(fd, offset)?;
        }

        match self.lfs.write(fd, &write_data) {
            Ok(written) => {
                // Update tracking
                let syn_file = self.files.get_mut(&path).unwrap();
                syn_file.record_write(offset, written);
                self.lfs.close(fd)?;
                Ok(written)
            }
            Err(e) => {
                self.lfs.close(fd).ok();
                // If write failed, remove from tracking if it was newly created
                if self.files.get(&path).is_none_or(|f| f.size == 0) {
                    self.files.remove(&path);
                }
                Err(e)
            }
        }
    }

    /// Branch 2: Remove a random file and clean up empty directories.
    fn branch_remove(&mut self) -> bool {
        if let Some(path) = self.select_random_file() {
            let lfs_path = path.trim_start_matches('/');
            if self.lfs.remove(lfs_path).is_ok() {
                self.files.remove(&path);
                self.cleanup_empty_dirs(&path);
                return true;
            }
        }
        false
    }

    /// Verifies a file's content matches expected.
    fn verify_file(&mut self, path: &str) -> Result<(), String> {
        let syn_file = match self.files.get(path) {
            Some(f) => f,
            None => return Ok(()), // File was removed
        };

        if syn_file.size == 0 {
            return Ok(()); // Empty file, nothing to verify
        }

        let expected = syn_file.generate_content(0, syn_file.size as usize);
        let lfs_path = path.trim_start_matches('/');

        let fd = self
            .lfs
            .open_file(lfs_path)
            .map_err(|e| format!("Failed to open {}: {:?}", path, e))?;
        self.lfs
            .seek(fd, 0)
            .map_err(|e| format!("Seek failed: {:?}", e))?;

        let mut actual = vec![0u8; syn_file.size as usize];
        let n = self
            .lfs
            .read(fd, &mut actual)
            .map_err(|e| format!("Read failed: {:?}", e))?;
        self.lfs.close(fd).ok();

        if n != expected.len() {
            return Err(format!(
                "Size mismatch for {}: got {}, expected {}",
                path,
                n,
                expected.len()
            ));
        }

        if actual != expected {
            // Find first mismatch
            for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
                if a != e {
                    return Err(format!(
                        "Content mismatch for {} at byte {}: got {}, expected {}",
                        path, i, a, e
                    ));
                }
            }
        }

        Ok(())
    }

    /// Runs the stress test until target_bytes have been written.
    fn run(&mut self) {
        let mut no_space_streak = 0;

        println!(
            "Starting stress test, target: {}",
            format_size(self.target_bytes)
        );
        println!(
            "Filesystem size: {} blocks ({})",
            self.fs_blocks,
            format_size((self.fs_blocks * BLOCK_SIZE) as u64)
        );

        while self.total_bytes_written < self.target_bytes {
            // Report progress every 1 GiB
            if self.total_bytes_written - self.last_progress_bytes >= 1024 * 1024 * 1024 {
                let progress_pct =
                    (self.total_bytes_written as f64 / self.target_bytes as f64) * 100.0;
                println!(
                    "Progress: {:.2}% ({} written, {} writes, {} removes, {} files)",
                    progress_pct,
                    format_size(self.total_bytes_written),
                    self.write_count,
                    self.remove_count,
                    self.files.len()
                );
                self.last_progress_bytes = self.total_bytes_written;
            }

            let usage_pct = self.lfs.usage_percent();

            // Run cleaner proactively when at 75% usage
            if usage_pct >= CLEANER_THRESHOLD_PERCENT {
                match self.lfs.clean() {
                    Ok(reclaimed) => {
                        if reclaimed > 0 {
                            println!(
                                "Proactive clean at {}% usage, reclaimed {} blocks",
                                usage_pct, reclaimed
                            );
                        }
                    }
                    Err(e) => {
                        panic!("Cleaner failed: {:?}", e);
                    }
                }
            }

            // Block writes at 80% usage - must remove files to make progress
            if usage_pct >= WRITE_BLOCK_THRESHOLD_PERCENT {
                if self.branch_remove() {
                    self.remove_count += 1;
                } else {
                    // No files to remove and at 80% - run cleaner
                    match self.lfs.clean() {
                        Ok(reclaimed) => {
                            if reclaimed == 0 {
                                no_space_streak += 1;
                                if no_space_streak > 1000 {
                                    panic!(
                                        "Cannot make progress at {}% usage: {} files, cleaner reclaimed 0 blocks",
                                        usage_pct,
                                        self.files.len()
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            panic!("Cleaner failed: {:?}", e);
                        }
                    }
                }
                continue;
            }

            // Reset no_space_streak when below threshold
            no_space_streak = 0;

            // Normal operation: flip a coin between create and remove
            let do_create = coin()(&mut self.guac) || self.files.is_empty();

            if do_create {
                match self.branch_create_and_write() {
                    Ok(written) => {
                        self.total_bytes_written += written as u64;
                        self.write_count += 1;
                    }
                    Err(Error::NoSpace) => {
                        // Hit NoSpace - run cleaner to free space
                        match self.lfs.clean() {
                            Ok(_) => {}
                            Err(e) => {
                                panic!("Cleaner failed: {:?}", e);
                            }
                        }
                    }
                    Err(Error::FileTooLarge) => {
                        // Skip this file, it's too large
                    }
                    Err(e) => {
                        panic!("Unexpected error during write: {:?}", e);
                    }
                }
            } else if self.branch_remove() {
                self.remove_count += 1;
            }
        }

        println!(
            "\nStress test complete! {} written in {} writes, {} removes",
            format_size(self.total_bytes_written),
            self.write_count,
            self.remove_count
        );

        // Final verification of all remaining files
        println!("Verifying {} remaining files...", self.files.len());
        let paths: Vec<String> = self.files.keys().cloned().collect();
        for path in &paths {
            if let Err(e) = self.verify_file(path) {
                panic!("Verification failed: {}", e);
            }
        }
        println!("All files verified successfully!");
    }
}

fn main() {
    let config = match Config::from_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    println!("Running synfs-stress-test with seed {}", config.seed);
    let mut test = StressTest::new(&config);
    test.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_bytes() {
        assert_eq!(parse_size("100").unwrap(), 100);
        assert_eq!(parse_size("0").unwrap(), 0);
        assert_eq!(parse_size("1234567890").unwrap(), 1234567890);
    }

    #[test]
    fn parse_size_kib() {
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("100K").unwrap(), 100 * 1024);
    }

    #[test]
    fn parse_size_mib() {
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1m").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("512M").unwrap(), 512 * 1024 * 1024);
    }

    #[test]
    fn parse_size_gib() {
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1g").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("4G").unwrap(), 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_tib() {
        assert_eq!(parse_size("1T").unwrap(), 1024 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1t").unwrap(), 1024 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_whitespace() {
        assert_eq!(parse_size("  100  ").unwrap(), 100);
        assert_eq!(parse_size(" 1K").unwrap(), 1024);
    }

    #[test]
    fn parse_size_invalid() {
        assert!(parse_size("").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("1X").is_err());
    }
}
