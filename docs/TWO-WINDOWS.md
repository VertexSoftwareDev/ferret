# Two windows, one engine

Ferret ships two front ends over the same code.

| | `ferret-native` | `ferret-app` |
|---|---|---|
| Draws with | [egui], straight to OpenGL | WebView2, through [Tauri] |
| Interface written in | Rust | HTML, CSS, JavaScript |
| Ships as | one `.exe` | an `.exe`, and WebView2 must be installed |

Everything below the window is shared. [`ferret-core`](../crates/ferret-core)
reads the master file table and answers queries; [`ferret-shell`](../crates/ferret-shell)
holds the indexes, runs the searches, formats the rows and tails the change
journal. Neither window contains a line of NTFS logic, and neither has its own
copy of the change-journal rules — which is the point, because the moment those
diverge one window starts showing a truth the other does not.

Both windows do the same things: same five columns, same filters, same sort,
same keyboard, same Turkish and English, same live updates, same colours.

## Why there are two

The honest answer to "is a Tauri app a real desktop application?" is yes. It is
a native executable with a native window, no server and no internet, and
WebView2 is only the surface it draws on. VS Code, Discord and Obsidian are
built the same way, and nobody calls those web pages.

But a file search whose entire argument is that it beats what Windows ships has
an awkward spot when it starts a browser engine to draw a list of names. So the
same application was written a second time without one. Keeping both is worth
more than picking a winner: the engine now has two independent users, which is
the cheapest way there is to find out which parts of it were secretly about the
front end.

## What it costs

Measured on this machine — Windows 11, a 1.44-million-file NTFS volume, release
builds, each started from cold and left to settle for five seconds after the
index was ready. Two runs each; where they differed, both are shown. The
WebView2 figures count only the processes that launch started, not the ones
other applications on the machine were already running.

| | `ferret-native` | `ferret-app` |
|---|---|---|
| Processes | **1** | 7 |
| Working set | **246 MB** | 564–572 MB |
| Private bytes | **312–369 MB** | 506–557 MB |
| Window on screen | 94–120 ms | **46 ms** |
| Indexed and searchable | 7.6–9.3 s | 7.7–9.0 s |
| Executable | 6.6 MB | **4.2 MB**, plus WebView2 |
| Crates compiled | **136** | 239 |

The index itself accounts for 155 MB of the memory in both columns, so the
window's own cost is roughly **90 MB against 410 MB** — the browser engine is
about four and a half times the application it is drawing.

Time to index is identical, as it should be: that number belongs to the engine,
and this table is only measuring what is wrapped around it.

The one column the web view wins is the first frame. It puts an empty window on
screen in 46 ms and fills it once WebView2 is up; egui builds its OpenGL context
first and shows a finished window. Neither is a wait anyone notices, and the
native one is a window with content in it.

## What it costs to write

| | `ferret-native` | `ferret-app` |
|---|---|---|
| Interface code | 2,644 lines of Rust (330 of them tests) | 172 lines of Rust + 1,869 of HTML, CSS and JavaScript |
| Tested by | `cargo test` | `node scripts/check-i18n.js`, and the eye |

More lines, in one language, under one test runner. The interesting difference
is not the count but what is checkable: the native window's string tables, its
case-insensitive highlighting, its theme contrast and its request collapsing are
ordinary Rust functions with ordinary tests. The equivalents in `ui/app.js` are
reachable only by opening the window and looking.

Against that, CSS is genuinely better at being CSS. The web view's layout,
theming and transitions are shorter and easier to change than their egui
counterparts, and adding a fifth column is a two-line change there and a
ten-line one here.

## Where they actually differ

Things worth knowing before choosing one.

**Fonts.** The native window loads Segoe UI from the Windows font folder, which
covers Latin, Greek, Cyrillic and the accented characters that turn up in real
file names. It has no Chinese, Japanese, Korean or Arabic glyphs, so names in
those scripts fall back to egui's bundled font and can show empty boxes. The web
view has the entire system font stack and shows everything.

**The result list.** egui's table is virtual by construction — it calls back only
for the rows on screen. The web view's list is virtual because roughly a hundred
lines of `ui/app.js` make it so, with a spacer element giving the scrollbar its
height and a translated container holding the visible rows. Both work; only one
had to be built.

**Selecting text.** A row in the web view is DOM, so a path can be selected with
the mouse and dragged into another window. In the native one a path is pixels;
copying goes through the context menu or `Ctrl+C`.

**Accessibility.** The web view is a document, so screen readers understand it
for free. egui speaks AccessKit, which is real but thinner.

**Where preferences live.** `localStorage` for one, an eframe storage file under
the user's application data for the other. Both survive a restart; neither is
readable by the other, so the two windows do not share a remembered drive.

## Running them

```powershell
cargo run --release -p ferret-native
cargo run --release -p ferret-app
```

Both accept `--selftest`, and both run exactly the same checks, because that
code lives in the shared crate too.

[egui]: https://github.com/emilk/egui
[Tauri]: https://tauri.app
