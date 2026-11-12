//! Hourly heap-profile summary of the MG daemon, so a heap that grows on an
//! operator host shows which code holds the memory.
//!
//! The binary is built with jemalloc profiling, and [`MALLOC_CONF`] turns it
//! on but leaves sampling off, so the seats, which run the same binary, pay
//! nothing. The daemon switches sampling on at startup. jemalloc then records
//! the stack of about one allocation per 512 KiB allocated (its default
//! `lg_prof_sample`). Every hour the daemon dumps the live samples, keeps the
//! frames of the code that asked for the memory, and reports the sites that
//! hold the most estimated live bytes.
//!
//! A site is a list of function names from the binary's symbol table. Frame
//! addresses and the dump file stay local.
//!
//! [`MALLOC_CONF`]: crate::MALLOC_CONF

use std::collections::HashMap;
use std::ffi::{CString, c_void};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Past startup allocation, so the first report shows steady-state holders.
const FIRST_REPORT_DELAY: Duration = Duration::from_secs(10 * 60);
const REPORT_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Sites reported per dump, by estimated live bytes.
const TOP_SITES: usize = 10;
/// Frames kept per site, innermost first, after the allocator frames.
const SITE_FRAMES: usize = 6;
/// Generic Rust names can be long. The start names the function.
const MAX_FRAME_CHARS: usize = 160;

/// Turn on heap sampling in this process and report the profile every hour
/// until the runtime shuts down. The dump is written to `dump_path` and
/// removed after it is read.
pub(crate) fn spawn(dump_path: PathBuf) {
    if let Err(errno) = set_sampling(true) {
        tracing::warn!(safe_to_share = true, errno, "heap profiling is unavailable");
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_REPORT_DELAY).await;
        let mut previous = HashMap::new();
        loop {
            let path = dump_path.clone();
            match tokio::task::spawn_blocking(move || read_profile(&path)).await {
                Ok(Ok(profile)) => {
                    report(&profile, &previous);
                    previous = profile.sites;
                }
                Ok(Err(failure)) => tracing::warn!(
                    safe_to_share = true,
                    failure = failure.as_str(),
                    "heap profile dump failed"
                ),
                Err(_) => return,
            }
            tokio::time::sleep(REPORT_INTERVAL).await;
        }
    });
}

/// Estimated live allocations of one site.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Live {
    bytes: f64,
    objects: f64,
}

impl Live {
    fn add(&mut self, other: Live) {
        self.bytes += other.bytes;
        self.objects += other.objects;
    }
}

/// Function names of one allocation site, innermost first.
type Site = Vec<String>;

#[derive(Debug, Default)]
struct Profile {
    sites: HashMap<Site, Live>,
    /// What this report itself holds, mostly the symbolizer's copy of the
    /// symbol table. It is reported apart, so it does not take a site slot.
    profiler: Live,
}

impl Profile {
    fn total(&self) -> Live {
        let mut total = Live::default();
        for live in self.sites.values() {
            total.add(*live);
        }
        total
    }

    /// Sites by estimated live bytes, largest first.
    fn top(&self, count: usize) -> Vec<(&Site, Live)> {
        let mut sites: Vec<_> = self
            .sites
            .iter()
            .map(|(site, live)| (site, *live))
            .collect();
        sites.sort_by(|a, b| b.1.bytes.total_cmp(&a.1.bytes));
        sites.truncate(count);
        sites
    }
}

fn report(profile: &Profile, previous: &HashMap<Site, Live>) {
    let total = profile.total();
    tracing::info!(
        safe_to_share = true,
        estimated_bytes = total.bytes as u64,
        estimated_objects = total.objects as u64,
        sites = profile.sites.len() as u64,
        profiler_bytes = profile.profiler.bytes as u64,
        "heap profile"
    );
    for (rank, (site, live)) in profile.top(TOP_SITES).into_iter().enumerate() {
        let before = previous.get(site).map_or(0.0, |live| live.bytes);
        tracing::info!(
            safe_to_share = true,
            rank = rank as u64 + 1,
            estimated_bytes = live.bytes as u64,
            estimated_objects = live.objects as u64,
            growth_bytes = (live.bytes - before) as i64,
            frames = %site.join(" < "),
            "heap profile site"
        );
    }
}

/// Why a dump could not be read. A fixed category, so it can be shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DumpFailure {
    Dump,
    Read,
    Parse,
}

impl DumpFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dump => "dump",
            Self::Read => "read",
            Self::Parse => "parse",
        }
    }
}

/// Dump the live samples to `path`, then read and symbolize them.
fn read_profile(path: &Path) -> Result<Profile, DumpFailure> {
    let dumped = dump(path);
    let text = dumped
        .map_err(|_| DumpFailure::Dump)
        .and_then(|()| std::fs::read_to_string(path).map_err(|_| DumpFailure::Read));
    let _ = std::fs::remove_file(path);
    let stacks = parse_dump(&text?).ok_or(DumpFailure::Parse)?;
    let mut names = HashMap::new();
    let mut profile = Profile::default();
    for stack in stacks {
        let frames: Vec<String> = stack
            .addresses
            .iter()
            .map(|&address| {
                names
                    .entry(address)
                    .or_insert_with(|| symbol(address))
                    .clone()
            })
            .collect();
        if frames.iter().any(|name| is_profiler_frame(name)) {
            profile.profiler.add(stack.live);
        } else {
            profile
                .sites
                .entry(site(frames.into_iter()))
                .or_default()
                .add(stack.live);
        }
    }
    Ok(profile)
}

/// The demangled function name at a return address, without its hash.
fn symbol(return_address: usize) -> String {
    let mut name = None;
    // The call instruction precedes the return address.
    backtrace::resolve(return_address.wrapping_sub(1) as *mut c_void, |symbol| {
        if name.is_none() {
            name = symbol.name().map(|name| format!("{name:#}"));
        }
    });
    let mut name = name.map_or_else(|| "?".to_owned(), |name| elide_generic_arguments(&name));
    if let Some((end, _)) = name.char_indices().nth(MAX_FRAME_CHARS) {
        name.truncate(end);
    }
    name
}

/// Replace the generic arguments of each path segment with `…`, so that
/// `<Core<T, S>>::poll` keeps its function name within [`MAX_FRAME_CHARS`].
/// The brackets of a qualified path, `<Type as Trait>`, stay.
fn elide_generic_arguments(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    // For each open bracket: whether it opens generic arguments.
    let mut brackets: Vec<bool> = Vec::new();
    let mut previous = None;
    for c in name.chars() {
        let inside_arguments = brackets.contains(&true);
        match c {
            '<' => {
                let arguments =
                    previous.is_some_and(|p: char| p.is_alphanumeric() || p == '_' || p == ':');
                if arguments && !inside_arguments {
                    out.push_str("<…");
                } else if !inside_arguments {
                    out.push(c);
                }
                brackets.push(arguments);
            }
            '>' if previous != Some('-') => {
                let arguments = brackets.pop().unwrap_or(false);
                if !brackets.contains(&true) && (arguments || !inside_arguments) {
                    out.push(c);
                }
            }
            _ if !inside_arguments => out.push(c),
            _ => {}
        }
        previous = Some(c);
    }
    out
}

/// Frames of the report itself: a stack through them is memory the report
/// holds, such as the symbolizer's cache.
fn is_profiler_frame(name: &str) -> bool {
    const READ: &str = concat!(module_path!(), "::read_profile");
    const SYMBOL: &str = concat!(module_path!(), "::symbol");
    name.starts_with(READ) || name.starts_with(SYMBOL)
}

/// The first [`SITE_FRAMES`] frames outside the allocator and the standard
/// containers, so that `Vec::push` is reported as the code that pushed.
fn site(frames: impl Iterator<Item = String>) -> Site {
    frames
        .filter(|name| !is_allocator_frame(name))
        .take(SITE_FRAMES)
        .collect()
}

fn is_allocator_frame(name: &str) -> bool {
    let name = name.strip_prefix("__rustc::").unwrap_or(name);
    let Some(qualified) = name.strip_prefix('<') else {
        return is_standard_path(name);
    };
    // `<Type as Trait>::method` is the type's code, such as a derived
    // `Clone`, unless the type is a primitive or the standard library's; then
    // the trait decides.
    match qualified.split_once(" as ") {
        Some((self_type, _)) if !is_standard_path(self_type) => false,
        Some((_, trait_path)) => is_standard_path(trait_path),
        None => is_standard_path(qualified),
    }
}

/// Whether a path names the allocator, the standard library, or a primitive.
fn is_standard_path(path: &str) -> bool {
    const CRATES: &[&str] = &[
        "alloc",
        "core",
        "std",
        "hashbrown",
        "tikv_jemallocator",
        "tikv_jemalloc_sys",
        "bool",
        "char",
        "str",
        "u8",
        "u16",
        "u32",
        "u64",
        "u128",
        "usize",
        "i8",
        "i16",
        "i32",
        "i64",
        "i128",
        "isize",
        "f32",
        "f64",
    ];
    // jemalloc internals, Rust's allocator shims, and C++ `operator new`.
    const SYMBOLS: &[&str] = &[
        "_rjem_", "je_", "prof_", "__rust_", "__rdl_", "__rg_", "_Znw", "_Zna",
    ];
    // C allocation entry points, jemalloc's own, and SQLite's wrappers.
    const FUNCTIONS: &[&str] = &[
        "malloc",
        "calloc",
        "realloc",
        "posix_memalign",
        "aligned_alloc",
        "mallocx",
        "rallocx",
        "do_rallocx",
        "sqlite3Malloc",
        "sqlite3MemMalloc",
        "sqlite3Realloc",
        "sqlite3MemRealloc",
        "mallocWithAlarm",
    ];
    let path = path.trim_start_matches(['&', '*', '[', '(', ' ']);
    let path = ["mut ", "const ", "dyn "]
        .iter()
        .fold(path, |path, prefix| {
            path.strip_prefix(prefix).unwrap_or(path)
        });
    let first = path
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or_default();
    CRATES.contains(&first)
        || FUNCTIONS.contains(&first)
        || SYMBOLS.iter().any(|prefix| first.starts_with(prefix))
}

/// One sampled stack of a dump, with its live allocations scaled up from the
/// samples.
#[derive(Debug, PartialEq)]
struct Stack {
    addresses: Vec<usize>,
    live: Live,
}

/// Parse jemalloc's `heap_v2` dump: a header with the sampling period, then
/// for each stack an `@` line of return addresses and a `t*:` line with the
/// live sampled objects and bytes of all threads.
fn parse_dump(text: &str) -> Option<Vec<Stack>> {
    let mut lines = text.lines();
    let period: f64 = lines
        .next()?
        .strip_prefix("heap_v2/")?
        .trim()
        .parse()
        .ok()?;
    let mut stacks = Vec::new();
    let mut addresses = None;
    for line in lines {
        if line == "MAPPED_LIBRARIES:" {
            break;
        }
        if let Some(rest) = line.strip_prefix("@ ") {
            addresses = Some(
                rest.split_whitespace()
                    .map(|address| usize::from_str_radix(address.strip_prefix("0x")?, 16).ok())
                    .collect::<Option<Vec<_>>>()?,
            );
        } else if let Some(rest) = line.trim_start().strip_prefix("t*:") {
            // The totals line before the first stack has no addresses.
            let Some(addresses) = addresses.take() else {
                continue;
            };
            let (objects, rest) = rest.split_once(':')?;
            let objects: f64 = objects.trim().parse().ok()?;
            let bytes: f64 = rest.split_whitespace().next()?.parse().ok()?;
            stacks.push(Stack {
                addresses,
                live: unsample(objects, bytes, period),
            });
        }
    }
    Some(stacks)
}

/// Scale sampled objects and bytes to an estimate of all live ones. An object
/// of `size` bytes is sampled with probability `1 - exp(-size / period)`; this
/// is the correction `jeprof` applies.
fn unsample(objects: f64, bytes: f64, period: f64) -> Live {
    if objects == 0.0 {
        return Live::default();
    }
    let probability = 1.0 - (-(bytes / objects) / period).exp();
    Live {
        bytes: bytes / probability,
        objects: objects / probability,
    }
}

/// Switch heap sampling on or off for this process. Fails with the
/// `mallctl` error number when the allocator was built without profiling.
fn set_sampling(active: bool) -> Result<(), i32> {
    let mut active = active;
    // SAFETY: `prof.active` takes a `bool`, and `active` outlives the call.
    let result = unsafe {
        tikv_jemalloc_sys::mallctl(
            c"prof.active".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            (&raw mut active).cast(),
            size_of::<bool>(),
        )
    };
    if result == 0 { Ok(()) } else { Err(result) }
}

fn dump(path: &Path) -> Result<(), i32> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| libc::EINVAL)?;
    let mut path_ptr = path.as_ptr();
    // SAFETY: `prof.dump` takes a `const char *` file name, and `path`
    // outlives the call.
    let result = unsafe {
        tikv_jemalloc_sys::mallctl(
            c"prof.dump".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            (&raw mut path_ptr).cast(),
            size_of::<*const std::ffi::c_char>(),
        )
    };
    if result == 0 { Ok(()) } else { Err(result) }
}

#[cfg(test)]
mod tests {
    use super::{
        Live, elide_generic_arguments, is_allocator_frame, is_profiler_frame, parse_dump,
        read_profile, set_sampling, site, unsample,
    };

    #[test]
    fn parses_stacks_and_scales_samples() {
        let dump = "heap_v2/524288\n  \
            t*: 3: 1572864 [0: 0]\n  \
            t0: 3: 1572864 [0: 0]\n\
            @ 0x10 0x20 0x30\n  \
            t*: 2: 1048576 [0: 0]\n  \
            t0: 2: 1048576 [0: 0]\n\
            @ 0x40\n  \
            t*: 1: 64 [0: 0]\n  \
            t1: 1: 64 [0: 0]\n\
            \n\
            MAPPED_LIBRARIES:\n\
            @ 0x99\n  \
            t*: 7: 7 [0: 0]\n";
        let stacks = parse_dump(dump).unwrap();
        assert_eq!(
            stacks.len(),
            2,
            "the totals line and the map section are not stacks"
        );
        assert_eq!(stacks[0].addresses, [0x10, 0x20, 0x30]);
        assert_eq!(stacks[1].addresses, [0x40]);
        // Two 512 KiB objects: each sampled with probability 1 - e^-1.
        let scale = 1.0 / (1.0 - (-1.0f64).exp());
        assert!((stacks[0].live.bytes - 1_048_576.0 * scale).abs() < 1.0);
        assert!((stacks[0].live.objects - 2.0 * scale).abs() < 1e-9);
        // A 64-byte object is sampled about once per 8192 such objects.
        let small = stacks[1].live.objects;
        assert!((8000.0..8400.0).contains(&small), "{small}");
    }

    #[test]
    fn rejects_a_dump_with_another_format() {
        assert_eq!(parse_dump("heap/524288\n"), None);
        assert_eq!(
            parse_dump("heap_v2/524288\n@ 0xzz\n  t*: 1: 1 [0: 0]\n"),
            None
        );
    }

    #[test]
    fn empty_sample_estimates_nothing() {
        assert_eq!(unsample(0.0, 0.0, 524_288.0), Live::default());
    }

    #[test]
    fn site_skips_allocator_frames_and_keeps_the_caller() {
        let frames = [
            "_rjem_je_prof_backtrace",
            "_rjem_je_malloc_default",
            "__rustc::__rust_alloc",
            "alloc::raw_vec::RawVecInner<A>::finish_grow",
            "<u8 as alloc::vec::spec_from_elem::SpecFromElem>::from_elem::<alloc::alloc::Global>",
            "<alloc::vec::Vec<T> as core::clone::Clone>::clone",
            "<&mut alloc::vec::Vec<T> as core::fmt::Write>::write_str",
            "<mg_core::seat::Seat as core::clone::Clone>::clone",
            "<alloc::vec::Vec<T> as mg_core::Encode>::encode",
            "mg_core::fleet::Fleet::seat_summary",
        ]
        .map(String::from);
        assert_eq!(
            site(frames.into_iter()),
            [
                "<mg_core::seat::Seat as core::clone::Clone>::clone",
                "<alloc::vec::Vec<T> as mg_core::Encode>::encode",
                "mg_core::fleet::Fleet::seat_summary"
            ]
        );
        assert!(!is_allocator_frame("mg_core::alloc_seat"));
        for c_frame in [
            "do_rallocx",
            "mallocx",
            "_Znwm",
            "sqlite3MemMalloc",
            "mallocWithAlarm",
        ] {
            assert!(is_allocator_frame(c_frame), "{c_frame}");
        }
        assert!(!is_allocator_frame("_ZN7rocksdb5Arena16AllocateNewBlockEm"));
        assert!(
            !is_allocator_frame("core_affinity::get"),
            "a crate named like core"
        );
    }

    #[test]
    fn elides_generic_arguments_but_keeps_qualified_paths() {
        assert_eq!(
            elide_generic_arguments(
                "<tokio::runtime::task::core::Core<mg::Task<Vec<u8>>, Arc<Handle>>>::poll"
            ),
            "<tokio::runtime::task::core::Core<…>>::poll"
        );
        assert_eq!(
            elide_generic_arguments("<alloc::vec::Vec<T> as core::clone::Clone>::clone"),
            "<alloc::vec::Vec<…> as core::clone::Clone>::clone"
        );
        assert_eq!(
            elide_generic_arguments("<u8 as SpecFromElem>::from_elem::<alloc::alloc::Global>"),
            "<u8 as SpecFromElem>::from_elem::<…>"
        );
        assert_eq!(
            elide_generic_arguments("mg::run::{closure#0}::<impl Fn() -> u8>"),
            "mg::run::{closure#0}::<…>"
        );
        assert_eq!(
            elide_generic_arguments("mg_core::seat::Seat::start"),
            "mg_core::seat::Seat::start"
        );
    }

    #[test]
    fn the_reports_own_frames_are_set_apart() {
        assert!(is_profiler_frame("fleet_manager::heap_profile::symbol"));
        assert!(is_profiler_frame(
            "fleet_manager::heap_profile::read_profile"
        ));
        assert!(!is_profiler_frame(
            "fleet_manager::heap_profile::tests::hold_blocks_for_heap_profile_test"
        ));
    }

    /// Holds 64 MiB in 1 MiB blocks, from a frame the profile must name. It
    /// pushes in a loop, so an optimized build cannot turn the allocation into
    /// a tail call that leaves this frame off the stack.
    #[inline(never)]
    fn hold_blocks_for_heap_profile_test(blocks: &mut Vec<Vec<u8>>) {
        for i in 0..64u8 {
            blocks.push(vec![i; 1 << 20]);
        }
    }

    #[test]
    fn a_held_allocation_is_the_top_site_with_its_size() {
        set_sampling(true).expect("this binary links jemalloc with profiling");
        let mut held = Vec::with_capacity(64);
        hold_blocks_for_heap_profile_test(std::hint::black_box(&mut held));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.heap");
        let profile = read_profile(&path).unwrap();
        assert!(!path.exists(), "the dump is removed after it is read");
        let (site, live) = profile
            .sites
            .iter()
            .find(|(site, _)| {
                site.first()
                    .is_some_and(|frame| frame.contains("hold_blocks_for_heap_profile_test"))
            })
            .unwrap_or_else(|| panic!("no site names the holder: {:#?}", profile.top(5)));
        let mib = live.bytes / f64::from(1 << 20);
        // 64 samples of 1 MiB: the estimate is exact up to sampling noise.
        assert!((40.0..90.0).contains(&mib), "{mib} MiB at {site:?}");
        drop(held);
    }
}
