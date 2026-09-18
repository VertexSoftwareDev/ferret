# How Ferret works

This is the long version of the README's "how it works" section: what the NTFS
master file table is, what makes parsing it awkward, and why the search is laid
out the way it is.

## Why read the master file table at all

The obvious way to index a disk is to walk it: open a directory, list it, recurse.
On a volume with 1.8 million files that is millions of `FindFirstFile` /
`FindNextFile` round trips, each one a kernel transition, most of them landing on
metadata scattered across the disk. It takes minutes.

NTFS already maintains the answer. Every file and directory on the volume owns at
least one record in a file called `$MFT`, and that file is one contiguous-ish run
of ~1 KB records holding names, parents, sizes and timestamps. Reading it is one
long sequential read — the access pattern storage hardware is best at.

On the test machine: **1.8 M records, about 2 GB of `$MFT`, read and parsed in
7 seconds**, roughly 300 000 records per second.

## Bootstrapping

`$MFT` is itself a file, so finding it is a small chicken-and-egg problem.

1. Open the volume as `\\.\C:`. This needs administrator rights, and every read
   must be aligned to the sector size — [`volume.rs`](../crates/ferret-core/src/volume.rs)
   widens unaligned requests rather than making callers care.
2. The boot sector at offset 0 gives the geometry: bytes per sector, sectors per
   cluster, the size of one MFT record, and the cluster where `$MFT` starts.
3. Read record 0 — the record describing `$MFT` itself. Its `$DATA` attribute is
   a **run list**, and decoding that gives every extent of the table.

## Data runs

A non-resident attribute does not store its content inline; it stores a list of
extents. Each entry is a length in clusters and an offset **relative to the
previous entry's start**, with the two field widths packed into the nibbles of a
leading byte.

```
0x21 0x18 0x33 0x02      length field 1 byte, offset field 2 bytes
     └ 0x18 clusters     └ starting at cluster 0x0233
```

Offsets are signed, so a fragmented file's runs can point backwards. A zero
offset field marks a sparse run — a hole with no storage behind it.
[`runs.rs`](../crates/ferret-core/src/runs.rs) decodes this and stops cleanly at
the first malformed entry rather than propagating a corrupt length.

## Records, and the fixup trick

Each record begins with `FILE` and a header, then a chain of attributes. Two
matter here: `$FILE_NAME` (0x30) with the name and the parent's record number,
and `$DATA` (0x80) with the size. `$STANDARD_INFORMATION` (0x10) supplies the
modification time and the hidden/system/read-only bits.

The catch is **fixups**. To detect writes that were torn halfway through, NTFS
takes the last two bytes of every sector in a record, replaces them with a single
check value, and stores the originals in an array in the header. Before a record
can be read, those bytes must be put back:

```
sector 0: [ ................................ CHECK ]
sector 1: [ ................................ CHECK ]
header:   usa = [ CHECK, real_bytes_0, real_bytes_1 ]
```

Miss this and every 512th byte pair of every record is wrong — which in practice
means names and offsets that are subtly, confusingly corrupt. Do it properly and
you get torn-write detection for free: if a sector does not end with the check
value, the record was not written atomically and is skipped.

## Names

Most files carry two `$FILE_NAME` attributes: the real long name, and a legacy
8.3 alias like `PROGRA~1`. They are distinguished by a namespace byte. Ferret
prefers `Win32` and `Win32AndDos` and ignores DOS-only aliases, because a result
list full of `MICROS~1` is useless.

## Reachability, and why it is a second pass

Some records must not appear in results:

- NTFS metafiles — `$MFT`, `$LogFile`, `$Bitmap` and the rest occupy records
  0–15, plus `$…` entries in the root.
- Their children. `$Extend` holds `$Quota`, `$ObjId`, `$RmMetadata` and the
  transaction logs beneath it.
- Genuinely broken chains on a damaged volume.

The tempting implementation is to drop a metafile and then drop anything whose
parent was dropped, as the scan goes. That is wrong, and the disk proves it:
**MFT record numbers are recycled**, so a child can occupy a *lower* record
number than its parent and be visited first. No forward pass can classify it.

So reachability is decided once the whole table is in memory: walk each entry's
parent chain, memoising the verdict, and keep only the entries that reach the
root. On the test volume that removes 605 records. The pass also rebuilds the
name arena and the record lookup around the survivors, so no dead weight is
carried.

## Memory layout

With two million entries, per-entry cost decides whether the app sits at 150 MB
or 400 MB.

```rust
pub struct Entry {           // 32 bytes
    pub record: u32,
    pub parent: u32,
    pub size: u64,
    pub modified: u64,       // Windows FILETIME
    name_offset: u32,        // into the index's shared name arena
    name_len: u16,
    pub flags: u16,          // dir / hidden / system / read-only
}
```

Names live in one shared `String`. The alternative — a `Box<str>` per entry —
means two million heap allocations, two million allocator headers, and a pointer
chase for every comparison.

Measured on the test volume: 108 MB for the index, 53 MB for the search arena.

## Search

The naive approach lowercases each name and asks whether it contains the needle.
On 1.8 M files that allocates 1.8 M strings per keystroke, and costs about
**100 ms** — far too slow to feel live.

Instead the lowercase form is built **once**, at scan time, into a single
contiguous arena with `\n` between names:

```
rapor.pdf\nnotlar.txt\nbelgeler\n…
```

NTFS forbids control characters in filenames, so `\n` is a separator that can
never occur inside one. A query is then one linear scan of ~50 MB, handed to the
two-way matcher behind `str::find`, split across every core at entry boundaries
so no name is cut in half. Each hit jumps to the end of its name, so a file whose
name contains the needle twice is still reported once.

That brings a query to **3–12 ms**.

### Several words, and wildcards

`setup exe` should find a name containing both, in any order, and `*.dll` should
mean what everyone expects. The query is split on whitespace — quotes put a
phrase back together — and every term has to match.

Testing every term against every name would undo the whole point of the arena,
so only one term is scanned for: the **longest literal run** anywhere in the
query, because that is the most selective thing available. `*.dll` scans for
`.dll`; `*yillik*rapor*` scans for `yillik`. The few names that come back are
then checked against the remaining terms, using the lowercase copy already in
the arena, so the check costs no allocation.

A query with nothing literal in it — `*`, `??` — has no anchor and falls back to
testing every name. That is the slow path, and it is the right trade: it is also
the rarest.

### Path queries

Matching against full paths would mean building a million strings. Instead a
query containing a separator is split in two: what follows the last separator is
matched against the name arena as usual, and what precedes it narrows the
folder. Only the survivors of the name match — usually a handful — have their
path built, and the folder half is compared against the *containing folder*
rather than the whole path, so a file's own name cannot accidentally satisfy it.

### Sorting

Sorting is applied after matching, not during, so a query returning half the disk
still answers instantly. Past 200 000 hits the sort is skipped and the UI says
so: at that size the list is not something anyone reads in order anyway.

## Staying current

A scan is true for one instant. NTFS keeps a circular log of every change — the
**USN journal** — and each entry names the MFT record it concerns, along with
that file's name, its parent and its attributes. A background thread per volume
polls it roughly twice a second through `FSCTL_READ_USN_JOURNAL`.

The obvious way to apply a change is to re-read the MFT record it names. That is
wrong, and quietly so. Ferret reads volumes **raw**, which bypasses the
filesystem cache: a file created a second ago still has an empty record on the
platter, so re-reading finds nothing and the change is lost. Measured on a test
that created fifty files, only two were picked up.

The journal entry already carries everything an index entry needs except the
size, so that is what gets applied — and the size comes from a normal
`metadata()` call on the reconstructed path, which does see the cache.

Three details keep this cheap:

- **Collapsing.** One save produces several entries for the same file: create,
  extend, close. They are reduced to the last state per record before anything
  is applied, which also avoids repeating the `metadata()` call.
- **Deleting without moving.** A deleted entry keeps its slot and gains a flag.
  Removing it would shift every later position and invalidate the prebuilt
  search arena; queries simply skip flagged entries.
- **Rebuilding rarely.** The arena only goes stale when a *name* appears or
  changes. Size and time updates leave it alone, and deletions are filtered, so
  the 100 ms rebuild happens at most once every 1.5 seconds and often not at
  all.

If the journal is disabled, or has wrapped past where Ferret was reading, the
index cannot be patched. The app says so and asks for `F5` rather than showing
results it can no longer vouch for.

## The application layer

Between the engine and the pixels sits `ferret-shell`: the indexes and the lock
around them, the queries, the row formatting, the change-journal watcher, and
the headless self-test. Nothing in it knows what a window is, and nothing in it
is async — every call blocks for as long as it takes and the caller decides
which thread pays.

That is what makes two front ends possible without two copies of the rules.
`ferret-native` draws with egui and is the one to reach for; `ferret-app` draws
the same application in a web view. Both are thin. The measured differences are
in [TWO-WINDOWS.md](TWO-WINDOWS.md).

Whichever window is on top, the same four things hold:

- Scanning and searching never run on the thread that draws. The native window
  hands them to a worker thread and takes the answers as events; the web view
  hands them to a blocking task and takes them as promises.
- The shell keeps the last query's hit list, so scrolling asks for a window of
  rows rather than re-running the search.
- A search takes a **read** lock and only the hit list it caches takes a write
  one, so two queries never wait on each other.
- The watcher rebuilds the search arena — 100 ms — under a read lock too, and
  takes the write lock only to swap the finished arena in. A generation counter
  on the volume tells it when the index moved underneath it, in which case the
  work is thrown away and retried on the next cycle. Typing stays responsive
  while files are changing.

Both windows virtualise the list, by different means. egui's table calls back
only for the rows on screen. The web view gets there by hand: a spacer element
gives the scrollbar its true height and a translated container holds the visible
rows, so a query matching a million files renders about thirty `div`s.

The native window adds one thing the web view does not need — requests are
collapsed before they run. A drag of the scrollbar queues forty page requests
and a fast typist queues six searches; only the last of each is still wanted,
and a search retires every page queued beside it, because those offsets point
into a hit list that is about to be replaced.

## Verifying it

Unit tests cover the parsers — fixup reversal, torn-record rejection, negative
run offsets, truncated run lists, attribute chains with corrupt lengths, 8.3
alias filtering, reachability and cycles, path building, and the search's
boundary cases.

The end-to-end check is `--selftest`, which both windows offer because it lives
in the shared crate. It scans a real volume, runs a set of queries, and then asks
Windows whether the paths Ferret reconstructed actually exist. Anything below
95 % agreement fails the run.
