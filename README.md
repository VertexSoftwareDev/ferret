# Ferret

Instant file search for Windows. Type a letter, and a million files filter in
under ten milliseconds.

Windows Search crawls your directories and keeps a database that is often stale
and frequently slow. Ferret takes the other route: it reads the filesystem's own
table of contents — the NTFS **master file table** — directly off the raw
volume. One sequential pass, and the whole disk is in memory.

```
  dosya            : 1550188
  klasor           : 295942
  tarama           : 7.99 sn
  arama indeksi    : 0.10 sn
  bellek           : 162.1 MB

  sorgu                                 sonuc       sure
  a                                   1105004    12.47 ms
  exe                                   17038     4.23 ms
  setup                                  3170     6.31 ms
  windows\system32\kernel                   1     5.10 ms
  zzqqxxnope                                0     3.67 ms

  yol dogrulama    : 200/200 yol diskte bulundu

  canli guncelleme
      olusturulan 25 dosyadan bulunan : 25
      silindikten sonra kalan             : 0

  SONUC: gecti
```

That is real output from `ferret-app.exe --selftest` on a 1.8-million-file
volume. It checks that every reconstructed path actually exists on disk, and
that files created and deleted while it runs appear and disappear from the
index on their own.

## Screenshots

Not checked in yet — grab them from your own disk, where the file names are real:
run the app, press `Win+Shift+S`, and drop the images in `docs/`.

## What it does

- **Indexes a whole NTFS volume in seconds** by parsing `$MFT` rather than
  walking directories.
- **Filters as you type.** Every query is a single linear pass over a prepared
  lowercase arena, split across all cores.
- **Searches paths too.** Type `belgeler\rapor` and only that folder's matches
  come back — without ever materialising a million path strings.
- **Sorts and filters** by name, size, date, kind, and hidden/system attributes.
- **Stays current.** Ferret tails the NTFS change journal, so files created,
  renamed or deleted while it is open show up within about a second — no
  re-scan.
- **Opens what you find**: double-click, Enter, reveal in Explorer, copy path.
- **Speaks English or Turkish**, switched from the toolbar and remembered. Only
  the interface changes: file names, paths and dates come from the disk exactly
  as the filesystem has them.
- **Reads only.** Ferret opens volumes for reading and never writes a byte back.

## Requirements

- Windows 10 or 11
- An NTFS volume
- Administrator rights — raw volume access is privileged. Ferret starts without
  them and offers to restart elevated at the moment it needs to index.

## Running it

```powershell
cargo run --release -p ferret-app
```

The development harness for the engine, which is where the numbers above come
from:

```powershell
cargo run --release -p ferret-cli -- scan  C     # index and summarise
cargo run --release -p ferret-cli -- find  C rapor
cargo run --release -p ferret-cli -- bench C     # query timings
```

And the end-to-end smoke test:

```powershell
cargo run --release -p ferret-app -- --selftest
```

## Keyboard

| Key | Action |
| --- | --- |
| `Ctrl+F` | Focus the search box |
| `↑` `↓` `PgUp` `PgDn` `Home` `End` | Move through results |
| `Enter` | Open the selected item |
| `Ctrl+C` | Copy its full path |
| `F5` | Re-index the current drive |
| `Esc` | Clear the query |

## How it works

`$MFT` is a table with one ~1 KB record per file. Each record holds the file's
name, its parent directory's record number, its size and its timestamps. Reading
that table start to finish gives you the entire filesystem without asking the
directory tree a single question.

Getting it right takes more than reading bytes in order:

- **Fixups.** NTFS replaces the last two bytes of every sector in a record with
  a check value and parks the real bytes in the header. Skip the reversal and
  every 512th byte pair is wrong; check it, and you also detect records that were
  torn mid-write.
- **8.3 aliases.** Most files carry two names, the real one and a legacy
  `PROGRA~1` alias. Indexing the alias makes results look like garbage.
- **Recycled record numbers.** A child's record can sit *below* its parent's, so
  no single forward pass can decide which entries are reachable. Reachability is
  settled after the scan, which also drops NTFS's own nested metafiles.
- **Search cost.** Lowercasing names per keystroke costs ~100 ms per query on a
  large volume. Lowercasing once into one contiguous arena — and searching that
  in parallel — costs 3–12 ms.
- **Stale snapshots.** The change journal names the record that changed, but
  re-reading that record from the raw volume returns stale bytes: raw reads
  bypass the filesystem cache, so a file created a second ago is still blank on
  the platter. The journal entry's own name, parent and attributes are the
  correct source.

There is a fuller walkthrough in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Layout

```
crates/ferret-core/   NTFS reader, index and search engine (no Windows API deps)
crates/ferret-cli/    Development harness: scan, find, bench
crates/ferret-app/    Tauri desktop application
ui/                   Front end: HTML, CSS, JavaScript — no framework
ui/i18n.js            Every string the interface shows, in both languages
scripts/              Icon generator
```

## Status

Working and measured on real volumes. Known limits:

- NTFS only. FAT32 and exFAT volumes are listed but cannot be indexed.
- Live updates need the volume's USN journal to be enabled, which it is by
  default on the system drive. Without it, Ferret keeps its snapshot until `F5`.
- Renaming a file appends its new name to the name arena and leaves the old
  bytes behind; a long session with heavy churn slowly grows memory until the
  next full scan.
- The command-line harness prints in Turkish; the application itself is
  bilingual.

## Licence

MIT. See [LICENSE](LICENSE).
