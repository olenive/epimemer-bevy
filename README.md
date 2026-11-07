# epimemer-bevy

A [Bevy](https://bevy.org) visualiser for [Epimemer](https://github.com/olenive/epimemer)
graphs: one 3D scene in which the knowledge graph stands on a map of its
topics and extends along a time axis, so a claim that held for years, a claim
pinned to a week and a claim whose period is only vaguely known look
different at a glance. The graph view, the timeline view and the free 3D view
are three camera presets of that one scene.

*Status: slices 1 to 10 of `DESIGN.md` §14, 2026-10-21. It draws
every node of one graph on a time axis, topics on a map made from their
embeddings, so that related topics stand together, and their claims
around them: in recorded time as a disc at the moment it was recorded, and
in claim time as the column, smear or disc of `DESIGN.md` §4, with a drawn
axis, its ticks and "now", what lies beyond the claim axis drawn clipped at
its ends, a tray of what claim time cannot place, and the
focused claim's validity brackets. The camera presets of `DESIGN.md` §6, a
free camera between them, and the filters of `DESIGN.md` §10, which dim or
hide nodes and are set from the bar along the top of the window or from the
command line, work in both modes. The bar also switches to another
session's or graph's snapshot. Every node can be hovered and focused, and
the labels of `DESIGN.md` §9 name a budget of topics and claims that
gathers around the focus. The edges of `DESIGN.md` §5 are drawn: a hovered
or focused node's claim-to-claim edges as lines, all of them at once with
the edge toggle, and every succession and merge as a connector. It follows
the graph's event stream, so a node, an edge, a status change or a
timeline stored on the server appears in the scene in the update it
arrives, without moving anything already drawn, and a stream that drops
or misses something is recovered by reading the graph again. A side
panel on the right shows the focused node in full, every field the hub
sends, its time in both modes and its edges by class, and a click on a
node it names focuses that one; a search field in the bar finds the
visible nodes whose text holds a word and walks them one by one. The
topic map is fitted once, from the hub's topic embeddings, and saved, so
the same map comes back on every start; a topic that arrives later is
put on it without moving any other, and Recalculate layout fits a new
one, as [The topic map](#the-topic-map) describes. The
same code builds for a browser, drawn with WebGL2 and served beside the
hub, as [In a browser](#in-a-browser) describes. `DESIGN.md` is the
design and its §14 the order of work.*

It is a read-only client of Epimemer's visualisation hub and depends on
nothing else: the routes, the snapshot and the event stream it reads are
specified in the Epimemer repository's `dev-docs/VIZ_CONTRACT.md`, and the
schema version it speaks is 1.

## Running

Start an Epimemer MCP server, which starts the hub on `127.0.0.1:8765`, then:

```
cargo run -- --graph memory
```

It follows the connected session whose active graph is the one named, or
the most recently active connected session, and `--session` names one
instead. `--hub HOST:PORT` reaches another hub. `--preset` starts at a
preset other than the 3D one: `graph`, `timeline` or `now`. `--time
claim` starts in claim time rather than recorded time. `--focus` starts
with a node focused, by its id or, for a topic, its exact name.
`--search TEXT` starts with TEXT typed into the search field and its
first hit focused.

The bar and the labels are drawn in Inter, from `assets/fonts/`, which Bevy
finds under `cargo run`. A binary run from elsewhere logs that it cannot
find the font and draws every text in Bevy's built-in font, which covers
ASCII only; `BEVY_ASSET_ROOT` set to this repository's directory finds it
again.

In the window:

| Key | What it does |
|---|---|
| `1` | Graph: straight down on the topic map, orthographic |
| `2` | Timeline: side-on with time up, orthographic |
| `3` | 3D: the whole graph from above and to one side, perspective |
| `N` | Now: the latest day's layer in recorded time, the layer at "now" in claim time, from the 3D direction |
| `R` | Reset: the 3D view of the whole graph, at the starting speed |
| `F` | Fit the whole graph, keeping the direction and projection |
| `M` | Fit the next metacontext, keeping the direction and projection |
| `T` | Fit the focused node's topic and its claims, keeping the direction and projection |
| `H` | Show or hide the bar |
| `L` | Show or hide every claim-to-claim edge between visible nodes, the edge toggle |
| `C` | Switch the time axis between recorded time and claim time |
| `P` | Show or hide the side panel |
| `J` | Show the log of acts, advisories and pipeline runs in the side panel, or return the panel to the focused node |
| `B` | Focus the node focused before this one, back along the focus history |
| `/` | Type into the search field, which then takes the keyboard |
| `Enter`, in the search field | Focus the next hit |
| `Shift` `Enter`, in the search field | Focus the hit before |
| `G` | Fit the current search hit's topic and its claims, as `T` does |
| `U` | Recalculate the topic map from the topics' embeddings and save it, as the bar's Recalculate layout does; every topic moves |
| Left click on a disc or a column | Focus its node |
| Left click on a node the side panel names | Focus that node |
| Left click on the scene where no disc is | Clear the focus |
| `Escape` | Leave the search field, close the bar's open list, or clear the focus |
| Right drag | Look around |
| `W` `A` `S` `D` | Move along the view; in an orthographic view `W` and `S` zoom |
| `Q` `E` | Move down and up |
| Scroll wheel | Move slower or faster; over the bar or the side panel, scroll it |

A change of view takes half a second, and a key during one retargets it.
Every view is fitted to the part of the window below the bar and, while
the side panel is open, left of it.
`cargo run -- --help` prints the same list, from the key map in
`src/keys.rs`.

### The topic map

The topics stand on a map of what they are about (`DESIGN.md` §7). On the
first start for a graph the topics are drawn on a circle while the client
reads their embeddings from the hub (`GET /api/embeddings`, the topic
vectors only), projects them to two dimensions by principal component
analysis, scaled so that the bulk of the map is the size of the circle,
and relaxes the result briefly, so that no two topics overlap and each
has room for some of its claims. The topics then move from the circle to
the map over a second and a half, their claims with them, and the view is
fitted again. Every claim keeps its place around its topic, an angle from
a hash of its id and a distance from its rank; on the map the ranks stand
closer together, by a factor the map's spacing sets (about a half on the
development graph), so that a busy topic's claims do not swallow its
neighbours.

The map is saved, natively as a small file per graph,
`~/Library/Application Support/epimemer-bevy/HOST%3APORT/GRAPH.json` on
macOS and under the XDG data directory (`~/.local/share/epimemer-bevy`)
elsewhere, and in a browser in the page's local storage. It holds each
topic's place by id, the fitted projection and the embedding model's name,
so on the next start the saved map is drawn at once and the picture never
jumps. The embeddings are still read then, and a topic the map does not
know is projected through the saved projection and put in the nearest free
space on the map; nothing else moves. A topic stored while the app runs
waits on a ring at the map's edge until its embedding is read, two seconds
after the last new topic of a burst, and one the hub has no embedding for
stays there; the side panel says which. When the hub reports another
embedding model than the saved map's, the map is fitted again by itself
and the bar says why, since a changed embedder changes every vector.

Recalculate layout, in the bar or `U`, reads the embeddings again, fits a
new map over the topics drawn, saves it over the old and moves every topic
to it. The fit is deterministic, so with the same topics it gives the same
map. `--recalculate` does the same at the start, and `--no-persist` reads
and saves no map at all. If the hub has no embeddings route, an Epimemer
older than the contract, or the read fails, the topics stay on the circle,
or on the saved map, and the bar says why.

### Focus, hover and labels

The disc under the pointer is drawn lighter, and the focused one lighter
still and wider. With a node focused, every other disc fades with its
distance from it, to about a third of its alpha some 60 units away, a
difference in time counting half as much as one on the map, so that the
focus's neighbourhood stands out and the rest of the graph stays in view.
`T` fits the topic of the focused node, the topic itself or the one a claim
was placed beside. The side panel shows the focused node in full, and `B`,
or its Back button, focuses the one before it, up to 32 steps back.

The labels are a budget, about one per 40,000 square pixels of the window,
23 in the default 1280 by 720, between 8 and 120. They go to the nodes
nearest the focus, or the point the camera looks at when nothing is focused,
topics before inferences before facts, and busier nodes first. Four in ten
are kept for topic names, so the map's place names are always there to
read. A label is placed to the right of its disc, to its left, above or
below, whichever overlaps no better-ranked label, and a node whose label
fits nowhere gets none, so no two labels overlap. The focused and the
hovered node are always labelled. Later labels in the ranking are fainter.
A label shows a topic's name or a claim's text, cut at forty characters.
Nodes the filter dims are labelled after every node that passes, and
hidden nodes never.

### Edges

The claim-to-claim edges, `supports`, `assessed`, `related`, `similarity`
and the rest of that class, are lines, drawn while one of their nodes is
hovered or focused: a straight line from the node's disc to the disc at the
other end, each at its own height, so in recorded time a line climbs from
the earlier claim to the later one. The focused node's lines are wider and
brighter than the hovered node's and stay until the focus changes, and the
discs at their far ends brighten a little with them. A line is as faint as
the fainter of its two discs, so a line to a node the filter dims is dim,
and a line to a node the filter hides, or to one the snapshot does not hold,
is not drawn. Each relation has its colour: `supports` green, `assessed`
orange, `related` violet, `similarity` cyan, `contradiction` red, and any
other type grey.

`L`, or the bar's Edges button, draws every claim-to-claim edge between two
visible nodes at once, for a look at the whole web. `--all-edges` starts
with it on.

A succession or merge edge, where one node was corrected by, followed by
or merged into another, is always drawn, as a connector from the old node
to the new: down the new node's axis to the old node's height and across to
it. The snapshot holds active nodes only, so the old node is usually not
there to draw; then the connector is a short stub down from the new node to
a small cross, which marks the node as one with a history. A connector to a
node the filter hides stops short, pointing toward it. Corrections,
world-changes and merges are drawn near white; a fact to the inference that
depended on it, when the fact was corrected or merged, in gold. In the
timeline view the connectors and stubs stand as verticals under the nodes.

Membership edges, a node's topic, tags, metacontext and source, are never
lines: they place the node and feed the filters. A `timelink`, which runs
to a timeline, is never a line either: it places its claim in claim time.

In claim time a line between two columns runs level, at the middle of the
stretch of time the two share, or where they share none, from the end of
the one nearer the other to the end of the other nearer it, so a line
between two discs still runs from disc to disc. A connector leaves the old
node at the top of its column and reaches the new node at the foot of its
column.

### Time

The time axis is calendar time in both modes, and the modes differ in where
a node is placed on it. In recorded time every node is a disc at the moment
it was recorded, so the scene shows the graph growing. In claim time a
claim is placed by the timepoint its `timelink` names on the snapshot's
timeline:

| Timepoint | Drawn as |
|---|---|
| `interval` | A solid column from its start to its end, at least a disc's height |
| `instant` | A thin disc at its start |
| `vague` with both bounds | A smear from the earliest to the latest, whole in the middle and fading toward each end |
| `vague` with one bound | A smear from that bound, whole there and fading out over a tenth of the axis toward the side that is not known |
| `vague` with neither bound, contested, or a kind this client does not know | Not drawn; listed in the tray |
| No `timelink` at all | A disc at its recorded time, never a column |

Topics stay discs at their recorded time in both modes. A claim linked to
several timepoints is drawn by the one that starts earliest, and the tray
lists the others. The recorded axis spans every node and is 300 units high.
The claim axis spans the bulk of what claim time places, so that one far
timepoint does not squeeze the rest into a band: of every placed instant,
the starts and ends of the columns, the instants, the bounds of the smears
and the recorded times of the discs, the lowest and the highest hundredth
are left out, the range is never shorter than a day, and it is widened
outward to whole units of the ticks, months, say, or years. It keeps the recorded axis's scale while it
can and stops at 600 units. On the development graph the claim axis runs
from 1 July to 1 November 2026, where an interval from 1372 to 1399 once
stretched it to six and a half centuries. A graph whose claims have no
timepoint with a position draws claim time on the recorded axis.

What lies beyond the claim axis is drawn clipped, at the end it lies
beyond: a column that starts before the axis is drawn from its foot, one
wholly before it is a flat disc at the foot, and the same at the top, each
with a small arrow in the axis's colour pointing out of the axis. The axis
has a break mark at an end that clips something, with the count of the
instants beyond it, such as "16 earlier" at the foot or "2 later" at the
top; a column's start and end count as two. A clipped node can be hovered
and focused like any other, and the side panel gives its true dates. Once
drawn the claim axis never rescales: a node recorded after its top grows it
upward, and a timepoint beyond it arriving later is clipped.

`C`, or the bar's Recorded and Claim buttons, switch the mode: every node
moves to its place in the other mode over half a second, the time window
keeps its stretch of calendar time, and the camera stays where it is.

The axis stands outside the corner of the map, at the left of the
timeline and 3D views, with ticks at centuries, decades, years, months,
weeks or days, the finest that leave room between them on the screen, so
zooming in brings finer ticks. "Now", the timeline's reference time or the
wall clock where it states none, has a green tick and a faint ring around
the map at its height; in claim time a "now" beyond the axis stands at its
end, and its label says so.

In claim time the side panel ends with the tray. It lists first the nodes
beyond the axis, those drawn clipped, with their dates; then the claims
linked to a timepoint with no position and why, and the claims linked to
more than one timepoint with the ones left out. A click on its heading
closes or opens it, and a click on a row focuses that node, which is how to
reach a claim that claim time does not draw; the focused node's own
timepoints and validity are in the panel's time section. A focused
claim's `sourced_from` validity intervals are
drawn as thin brackets beside its column: from a precise start to a
precise end with a tick at each, and where an end is named, unknown or
unbounded, running on past the precise end for eight units, half of them
dashed, with no tick.

### The bar

Along the top of the window, left to right:

| Control | What it does |
|---|---|
| Session, Graph | Pick a session the hub lists, by its short id, backend and active graph, and one of its graphs; a disconnected session is shown and cannot be picked. Picking a graph loads its snapshot in the background, keeps the kinds, metacontexts and dim or hide, and drops the topic, time window and source, which named things in the old graph. A load that fails says why at the end of the bar and leaves the graph drawn. |
| Stream | How the event stream stands: connected, with the messages received and the events applied; reconnecting, with the last error; or reading the graph again. |
| reflect | The graph drawn's reflect counter, `reflect N/M`: stores since its last reflect against the count at which reflect is suggested, with "suggested" once it is. A graph the session is not on shows `reflect -`, since the hub reports the active graph's only. |
| Log | Shows the log in the side panel in place of the focused node, and again returns to the node; `J` does the same. The log lists the session's acts and advisories of the graph drawn and its pipeline runs, newest first, filtered by kind, and a click on a node an entry names focuses it. |
| Time | Recorded time or claim time; `C` does the same. |
| Metacontexts | A checkbox for each of the graph's metacontexts, and All. |
| Kinds | Topics, facts and inferences, each on or off. |
| Status | Active, the only status the snapshot holds. |
| Topic | Pick a topic from the list, or type the start of its name to narrow the list; Enter takes the first. Clear drops it. |
| Source | Pick a document from the list, by the first eight characters of its id and the count of nodes read from it. Clear drops it. |
| Recorded, or Claim time | Drag either handle, or press the track, to set the time window over the axis drawn; the dates the handles stand for are beside them. In claim time it passes every column that reaches into it, the track spans the claim axis, and a window that reaches an end of it passes what is clipped there. Clear drops it. |
| Filtered out | Dim or hide the nodes that fail. |
| Edges | All, the edge toggle: every claim-to-claim edge between visible nodes drawn as a line; `L` does the same. |
| Search | A field: the nodes drawn whose content or description holds what is typed, ignoring case, with the count of them and which one is focused. Enter focuses the first hit and each further Enter the next, Shift and Enter the one before, wrapping at either end; the camera stays where it is. Go, or `G`, fits the current hit's topic. Clear empties the field and leaves the focus. |
| View | The views of the keys above: Graph, Timeline, 3D, Now, Reset, Fit, Metacontext and Topic, which fits the focused node's topic. |
| Recalculate layout | Fits the topic map again from the topics' embeddings, saves it and moves every topic to it; `U` does the same. |
| Hide bar | Collapses the bar to a thin strip, whose one button brings it back; `H` does both. |

Hovering a control says what it does at the end of the bar, a greyed-out
one why it waits, and otherwise that line names the graph, its nodes and
its topic map, or says why there is none. A list closes when an
entry is picked, on Escape, or on a press anywhere else, and the wheel
scrolls it. The topic list's field takes the keyboard only while the pointer
is over the bar, so with the pointer over the scene W, A, S and D always move
the camera. The search field takes it when it is clicked or `/` is
pressed, wherever the pointer is, and keeps it until Escape or a press
anywhere else; meanwhile the keys are typed into it and Enter walks its
hits. A node the filter hides, one claim time does not place, and one that
has left the active set are never hits, and the hits are found again
whenever the text, the filter or the graph changes. A click on the bar
never reaches the scene beneath it, and neither hovers nor focuses a disc.

### The side panel

On the right of the window, below the bar. With nothing focused it says
how to focus a node; with a node focused it shows, top to bottom:

- Every field the hub sends for the node, in plain words: its kind, the
  whole content and description, wrapped rather than cut as the label is,
  its status, its confidence or "unrated", how it was extracted, its source
  segment, when it was created and when a search last returned it, or
  "never returned by a search", its graph, its id, and its metadata as key
  and value rows.
- Membership: the topic it is filed under, or the tag it was placed by,
  and its tags, by name; its metacontexts by name and its source documents
  by id. A topic lists its subtopics, the topic it is a subtopic of, the
  topics it is tagged with and those tagged with it, and how many claims
  stand beside it.
- Claim to claim, and Succession and merge: each edge with its type and
  direction, `supports ->` from this node and `<- supports` into it, and
  the node at its far end by its label. A far end the snapshot does not
  hold, the old end of most successions, is named by its id from the edge
  and marked as not in the snapshot; one the filter hides is faint and
  marked as hidden. Each heading counts the edges and both kinds of far
  end.
- Time: where recorded time places it; in claim time the timepoint its
  column is drawn by, with its label, kind and bounds, the other
  timepoints it is linked to, and those with no place and why; and the
  validity intervals its sources assert, each end by its kind of instant,
  precise with its date, or named, unknown or unbounded, which have no
  coordinate and say so, and whether the source stated the dates or they
  were read from tense and context.

In claim time the tray follows. A click on a node the panel names focuses
it, so a chain of supports or successions can be walked, and Back or `B`
walks it back. The panel follows the event stream: a node changed in place,
or one that gains an edge, is shown again as it now is. It scrolls with
the wheel, takes the pointer as the bar does, so nothing beneath it is
hovered or clicked, and while it is open every view is fitted, and every
label placed, to its left. Hide, or `P`, collapses it to a button that
brings it back.

The snapshot carries no document names: the contract gives a document only
as the far end of a `sourced_from` edge. That is a known gap in it, and why
the source picker shows ids.

### Live updates

The client subscribes to the hub's event stream for the session and graph
it draws, before it reads the snapshot, and applies each event as it
comes (`DESIGN.md` §11):

- A new node is placed without moving any other. A topic takes the next
  place after the circle's last, keeping the circle's step on a ring just
  inside it, since a step past the last topic comes back round to the
  first. A claim stands in the unfiled region until its filing edge
  arrives, a moment later, and then moves beside its topic, the one case
  in which a node moves; within one update the move is never seen. A node
  recorded after the top of the time axis grows the axis upward at its own
  scale, so no height changes, and the axis, its dates and "now" follow.
  A node already drawn changes in place.
- A succession or merge connector appears at once, a claim-to-claim line
  on the next hover, and a membership edge updates what the filters read:
  the tags, the topic, the metacontexts and the sources.
- A node that leaves the active set is no longer drawn, and its connector
  ends in a marker, as for an old end the snapshot does not hold.
- A timeline stored again places the claims linked to it again in claim
  time.

A gap in the stream's `seq`, a reconnect of the stream, or the session
followed connecting to the hub again after it was seen disconnected,
reads the graph again in the background, over the picture drawn: every
node keeps its place and the axes their scale, and the focus, the hover,
the filter, the time mode and the camera are kept wherever what they name
is still in the graph. Events that arrive meanwhile are held and applied
after it. The stream tries a lost connection again after half a second,
doubling up to ten seconds. The acts, advisories and pipeline events go
to the log, the retrieval records to the Retrieval picker and the reflect
counter to the bar, each as it comes, the replay's among them, without
changing anything drawn. Events for another session or graph, and the
events this client does not use, are received and dropped.

### Filters from the command line

The bar's filters can be set at startup too. A filter dims the nodes that
fail it, so the shape of the whole graph stays in view; with `--hide` they
disappear instead, cannot be hovered or clicked, take no label, and every
view frames only what is left. A node must meet every filter given.

| Option | Shows |
|---|---|
| `--kinds topic,fact` | Only these kinds: `topic`, `fact`, `inference`. |
| `--metacontext the-real` | Only the nodes in these metacontexts, by id, separated by commas. |
| `--topic visualisation` | A topic, by id or exact name, the topics below it through `subtopic_of`, and every claim filed under or tagged with one of them; and one level out, every topic tagged with it and the claims filed under those. |
| `--window 2026-10-14..2026-10-20` | Only the nodes recorded in this stretch, or in claim time the columns that reach into it: ISO dates or timestamps in UTC, and a date at the end takes in that whole day. |
| `--source ID` | Only the nodes read from this document, the target of their `sourced_from` edges. |
| `--hide` | Hide the nodes that fail rather than dim them. |

An id or a topic name that no drawn node has is refused at startup with the
reason, since it is more likely mistyped than meant.

`--ui-state` sets how the bar starts: `open`, the default, `collapsed`, or
with a picker's list open, `sessions`, `graphs`, `metacontexts`, `sources`,
`retrievals`, `topics`, or `topics:PREFIX` with `PREFIX` typed into the
topic field; `log` starts with the log in the side panel.

A few options exist so a change can be checked with nobody at the window:
`--screenshot PATH` saves a PNG of the view `--preset` names, with the bar
as `--ui-state` sets it, the side panel and the labels, around the node
`--focus` names, or the first hit of `--search`, where one is given, once
the scene is drawn, the camera at rest, no switch of time mode under
way and the topic map settled, and exits; `--screenshot-after SECONDS` waits that long after the
start as well, so that a change made on the server meanwhile is in it;
`--fit-topic` then fits the focused node's topic first, as `T` does;
`--log-events` prints every event from the stream to stderr, one line
each: what it did to the picture and the counts after it, or why it was
dropped; and
`--fps` logs the frame rate and the draw calls the translucent phase took,
the discs and the line meshes, once a second (`--orbit` circles the camera
by itself while it does); `--layout-dir DIR` keeps the topic maps under
`DIR` rather than the data directory, so a check leaves the saved maps
alone; and `--dump-layout PATH` writes every topic's id, name and place on
the map to `PATH` as JSON once the map has settled, and exits, so that two
runs can be compared. `cargo run -- --help` lists them all.

`just check` runs the gates: the suite, clippy with warnings denied, the
formatting check, and the browser build's check, `just check-wasm`, which
compiles the crate for `wasm32-unknown-unknown` and runs clippy on it.
`just` lists the recipes.

## In a browser

The crate builds for the browser as WebAssembly, drawn with WebGL2, from
the same code as the native app. Only the transports differ: the page
reads the hub with its fetch and follows the event stream with its
WebSocket, as tasks on its event loop, where the native app uses a thread
for each. The browser build needs the `wasm32-unknown-unknown` target and
[trunk](https://trunkrs.dev):

```
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
```

With an Epimemer MCP server running, so that its hub answers on
`127.0.0.1:8765`:

```
trunk serve
```

and open `http://127.0.0.1:8080/`. Trunk builds the bundle, serves it,
rebuilds it on every change, and passes `/api` and `/ws` on to the hub
(`Trunk.toml`), so the page and the hub share an origin. That matters
because the hub sends no CORS headers, so a browser refuses its answers to
a page from any other origin. `just serve 8890` serves on another port.
The first build compiles Bevy for wasm32 and takes a few minutes, and
trunk fetches the `wasm-bindgen` and `wasm-opt` it needs into its own
cache. That development bundle is about 140 MB and quick to rebuild;
`trunk serve --release` serves the bundle as it is deployed, below, and
takes minutes to rebuild after each change.

A page has no command line, so its address gives the same options, with
the same defaults: `?graph=memory&preset=timeline&time=claim` reads as
`--graph memory --preset timeline --time claim`, and an option that takes
no value is given bare, `?hide&all-edges`, or as `1` or `true`. Every
option of the native app works but seven, which the page refuses with the
reason: `hub`, since the page reaches the hub at its own address, and the
native app's development options `screenshot`, `screenshot-after`, `fps`,
`log-events`, `layout-dir` and `dump-layout`. The page keeps its topic
maps in its local storage, under `epimemer-bevy/HOST:PORT/GRAPH`, so each
origin it is served from has maps of its own. `?help` lists the options and the keys. The keys and the mouse work as in the window once the canvas
has been clicked.

Until the graph is drawn the page says it is reading it, and if the start
stops, why: no hub answering, no connected session, a graph or an option
that names nothing. What the native app logs to the terminal goes to the
browser's console. The page runs on one thread, since it is served without
the cross-origin isolation a page needs for more, so the first graph is
placed on the page's own thread and the page shows only its line of text
meanwhile.

### Serving the bundle

`just wasm`, which runs `trunk build --release`, builds the bundle into
`dist/`: `index.html`, the bindings script, the `.wasm` shrunk with
`wasm-opt -Oz`, and `assets/`. The `.wasm` is 28.6 MB, 9.2 MB once
compressed with gzip and 6.0 MB with brotli. The `wasm-release` profile
in `Cargo.toml`, which optimises the whole program at once and for size,
takes it there from 45 MB, and makes a release build take about five
minutes. The bundle has to be served from the hub's own origin, for the
reason above, and there are two ways to do that:

- `trunk serve`, as above, which is for development.
- The hub serving `dist/` from a static directory, beside the dashboard it
  serves already, which needs no proxy. That is a change on the Epimemer
  side and is not made yet: a static mount on the hub's application, and
  the bundle built with `trunk build --release --public-url /PATH/` when
  it is mounted below the root.

A separate static server would need the hub to send CORS headers, which is
also an Epimemer change.

The bundle draws with WebGL2, which every current browser has. Bevy's
`webgpu` feature draws with WebGPU instead, but a bundle built with it has
no WebGL2 to fall back on, and the scene uses nothing WebGL2 lacks, so
this crate does not offer it.

## Licence

MIT, see `LICENSE-MIT`.
