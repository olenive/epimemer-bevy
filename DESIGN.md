# epimemer-bevy: design

Status: written on 2026-10-20 during the planning discussion in the
Epimemer repository and moved here the day this repository was created;
this file is the design's only home. Slices 1 to 10 of §14 are built
(2026-10-21), slice 11 waits on its Petritype prerequisites, and slices 12
to 15 were added on 2026-11-07, when the owner decided this client replaces
the dashboard (§1); slices 12 and 13 were built the same day. The numbers in §3 were measured on the development graph
on 2026-10-18, through the hub described in `VIZ_CONTRACT.md`, which is the
only surface this client depends on. `VIZ_CONTRACT.md` throughout means
`dev-docs/VIZ_CONTRACT.md` in the Epimemer repository.

The short version: one 3D scene in which the knowledge graph stands on a
topic map and extends along a time axis, three camera presets that are the
graph view, the timeline view and the free 3D view, a fixed set of filters,
a label budget instead of a label for every node, and a layout that does not
move under the user. Native first, wasm second.

## 1. The goal

The visualiser for Epimemer graphs, in Rust on Bevy, that is smoother
to move around in than the TypeScript dashboard and shows one thing the
dashboard cannot: time as a third dimension, so that a claim that held for
years and a claim pinned to a week look different at a glance, and a claim
whose period is only vaguely known looks different from both.

Why Bevy rather than a faster browser renderer: the user's other visual
work (`petritype-bevy`) is on this stack, the same code builds native and
for the browser, and the eventual activity bar (§13) reuses `petritype-bevy`
directly. The smoothness itself comes from drawing on the GPU with a layout
computed once, which any GPU renderer would give; Rust is the reason the
rest of the stack lines up, and is an honest preference.

Non-goals, for the first slices and until said otherwise:

- **Read-only.** The client records no verdict, archives nothing, changes no
  setting. The hub exposes no write, and this client asks for none.
- **Replacing the dashboard** stopped being a non-goal on 2026-11-07, when
  the owner decided this client replaces it. Its log, retrieval focus and
  pipeline views come here as slices 13, 12 and 11 (§14); its
  warnings-settings panel is not carried over, since the MCP tool
  `configure_warnings` is the interface and the log shows the advisories
  themselves. The dashboard leaves Epimemer in one cut once slices 12 and
  13 exist.
- **Native first.** The browser build came with slice 10 (§13); native
  stays the development target, where the suite runs.
- **No layout on the server.** Epimemer stays a data source (§12).

## 2. Decisions

Taken with the user on 2026-10-18, 2026-10-20 and 2026-11-07, not to be
re-litigated without them.

| Question | Decision |
|---|---|
| Where the code lives | Its own repository, `epimemer-bevy`: that name for the directory, the crate and the GitHub repository, matching `petritype-bevy`. |
| When it would move into the Epimemer repository | Only if Epimemer's core itself moves to Rust and a cargo workspace, Rust CI and a native build exist there for their own reasons. The move inward is cheap; the move outward is not. |
| Stack | Rust and Bevy 0.19. No Three.js prototype first; the hypothesis is tested in Bevy. |
| Menus, filters, panels | Bevy UI, kept to a short fixed set (§10). |
| Coupling to Epimemer | The contract in `VIZ_CONTRACT.md` and nothing else: HTTP and WebSocket JSON from the hub. Whether the server behind the hub is Python or Rust is invisible here, and the decision to port Epimemer's pipelines to `petritype-rs` is a separate one this client does not depend on. |
| Petritype crates | Enter only for the activity bar (§13), never for the graph scene. The knowledge graph is not a Petri net. |
| The three views | One scene, three camera presets (§6). |
| Translucent columns | Additive blending, accepted as the order-independent choice (§8). |
| Labels | A budget with greedy overlap rejection, not a label per node (§9). |
| Layout | Topic map from a 2D projection of topic embeddings, persisted; claims placed deterministically by their topic; recalculation only when the user asks (§7). |
| Edges | Membership becomes geometry, claim-to-claim edges are lines on hover and focus, succession and merge edges are always drawn, vertically (§5). No ribbons. |
| Time with no timepoint | A disc at recorded time, never a column (§4). |
| What Epimemer adds for this | A schema version and an embeddings route, both in the contract. No layout, no as-of snapshot. |
| The dashboard | Replaced by this client (2026-11-07), and removed from the Epimemer repository in one clean cut once slices 12 and 13 exist. The browser bundle is a release asset here, which `epimemer viz fetch` downloads into a directory the hub serves (§12, §14). |

## 3. The data as the client sees it

Everything comes from `GET /api/snapshot` and then the `/ws` stream, in the
shapes `VIZ_CONTRACT.md` §5 and §7 give. Measured on the development graph
on 2026-10-18:

| What | Count |
|---|---|
| Nodes | 2,026: 415 topics, 1,147 facts, 464 inferences, all `active` |
| Edges | 16,891 |
| Edges carrying a validity interval | 41, all `sourced_from` |
| Timelines | 1, with 84 timepoints: 55 `interval`, 29 `vague`, no `instant` |
| `timelink` edges | 475, of which 415 leave a claim; 397 distinct claims carry a timepoint |
| Snapshot size | 6.4 MB of JSON |

The edges fall into three classes, and the client treats each differently
(the full table with counts is `VIZ_CONTRACT.md` §5.3):

- **Membership**, about 13,800 edges: `tagged_with_topic`,
  `has_metacontext`, `sourced_from`, `contains`, `extracted_under_topic`,
  `implies`, `abstracts`, `about`. These say where a node belongs. They
  become position, colour and filter keys, never lines (§5).
- **Claim to claim**, about 2,050: `supports`, `timelink`, `assessed`,
  `related`, `similarity`. Drawn on demand (§5).
- **Succession and merge**, 192: `temporally_followed_by`, `merged_into`,
  `evidence_superseded`, `evidence_merged`, `superseded_by`. Always drawn
  (§5).

Three facts about the data that shape the design more than the counts:

1. **Several edge endpoints are not nodes.** `contains`, `about` and
   `implies` run from a segment, `sourced_from` runs to a document, and
   neither segments nor documents are in the snapshot. The client keeps
   those ids as provenance keys and draws nothing for them
   (`VIZ_CONTRACT.md` §5.1).
2. **The topic hierarchy is `subtopic_of`, and this graph has none.**
   `contains` is a segment-to-fact edge, not a topic-to-topic one. On the
   development graph the 415 topics are a flat set as far as edges go, so
   the topic map (§7) comes from the embeddings alone there, and the
   parent-child constraint applies only on graphs that have `subtopic_of`
   edges.
3. **Each claim has one filing topic.** A fact's is the `extracted_under_topic`
   edge (904 of 1,147 facts have one); an inference's is the `abstracts`
   edge (381 of 464). `tagged_with_topic` is the many-to-many layer over
   that, 7,024 edges, and is a filter rather than a position. A claim with
   no filing edge is placed by its first tag, and with no tag either, in an
   "unfiled" region at the map's edge.

**Retrieval records** come beside the graph rather than in it (slice 12).
A record is what one tool call handed the agent, in the field table below
`VIZ_CONTRACT.md` §4: `record_id`, `at`, `tool`, `query`, `graph`,
`retrieved`, `truncated` and `response_text`. `retrieved` lists at most 200
`{node_id, provenance, score}`, or is null where the tool declared nothing,
which differs from an empty list. `GET /api/retrievals?session=` answers the
session's 20 most recent records across every graph, oldest first; the
stream's `retrieval_recorded` event carries one as it is made, and the
replay on subscribe the last 20 of the graphs subscribed to (§11). Measured
on the development graph's session on 2026-11-07, the route answered 20
records in 78 KB, all of `epimemer-dev`, from six tools: seven declare
nothing (six `segment` calls and an `apply_reflection`), one declares an
empty list, and the other twelve name from 1 to 13 nodes.

## 4. Time

Two different times exist for every claim and the client offers both as the
third axis, switchable, with the same columns and the same layout:

- **Recorded time**, `created_at`. Every node has one and every edge has
  one. In this mode the scene shows the graph growing: a column is a disc at
  the moment the node was recorded, successions stack upward, and a day's
  ingest is a visible layer. This is the mode that works for a development
  history graph like the one measured, where three quarters of the claims
  have no claim time at all.
- **Claim time**, from the timeline. A node reaches a timepoint through a
  `timelink` edge whose `metadata.timepoint_id` names a point on the
  timeline in the snapshot. The point's `kind` decides the column:

| Timepoint | Column |
|---|---|
| `interval`, with `start` and `end` | A solid column from `start` to `end`. |
| `instant` | A thin solid disc at `start`. |
| `vague`, with `earliest` and/or `latest` | A smeared column across `[earliest, latest]`, alpha falling off toward each end; a missing bound fades out over a fixed distance rather than running to infinity. This is the case the third dimension exists for. |
| `vague` with neither bound, or `contested` | No position on the axis. Listed in a tray the side panel shows, the way the dashboard's timeline keeps them. |
| No `timelink` at all | **A disc at recorded time, never a column.** Unknown must not be drawn as forever: on the measured graph 1,214 of the 1,611 claims have no timepoint, and 1,214 infinite smears would drown the 397 that carry one. |

A `sourced_from` edge's validity interval is a third time, the period a
source asserts a claim held. It is on 41 edges here. In claim-time mode it
is drawn as a thin bracket beside the column, and only when the node is
focused, because its instants may be `named`, `unknown` or `unbounded` and
those have no coordinate either.

The axis is drawn with ticks at the granularity the visible span calls for,
and the timeline's `reference_time` (or the wall clock when it is null)
marks "now".

What the build settled (slice 7, measured on the development graph on
2026-10-20, when it had grown to 56 interval and 32 vague timepoints and no
instant). None of the 32 vague points has a bound, so none is a smear: the
20 claims linked only to them have no position, are not drawn in claim
time, cannot be picked, and are listed in the tray, from which a click
focuses them. Claim time draws 358 solid columns, 54 of the 56 interval
points among them, and 1,719 discs at their recorded time, the 430 topics
and the 1,289 claims with no `timelink`; the tray lists 38 claims, the 20
with no position and 18 drawn by the earliest of several timepoints. The 35
timelinks that leave a topic place nothing, since topics stay discs. Each
mode's axis spanned what it placed; the claim axis keeps the recorded axis's
scale while it can and stops at twice its height, 600 units. On this graph
one interval, 1372 to 1399, stretched the claim axis to six and a half
centuries, so the whole of 2026, where every other column lies, was less
than one unit at the top and read as a band. The clip below replaced that.
A missing vague bound fades out over a tenth of the axis's height; a smear
across two bounds keeps a quarter of its alpha at each so the bounds show;
an instant is 0.35 units thick against a disc's 1, and no column is drawn
shorter than a disc. Among several timepoints the earliest is the one with
the lowest stated bound. A claim-to-claim line between columns that share
no stretch of time runs from the nearer end of each. "Now" is the first
timeline's reference time, else the wall clock as the graph was loaded.
The four column meshes share one vertex layout and one material, so the
translucent phase stays at a handful of draw calls: five, against three
before, the axis's gizmos adding two, and five as well with 21 smears
mixed in, checked on a copy of the snapshot whose vague points were given
bounds. Of the 41 validity intervals, 27 run from a precise start to an
unknown end and so draw as open brackets; a bracket between two precise
days is a small fraction of a unit at this scale.

The clip (decided by the owner on 2026-10-21, built as a follow-up to slice
7 in `clip.rs`). In claim time the axis spans the bulk of what is placed.
Of every placed instant, the starts and ends of the columns, the instants,
the bounds of the smears and the recorded times of the discs, the lowest
and the highest hundredth, rounded down, are left out, so the range holds
the central 98%. A range shorter than a day grows to a day about its
middle, so that claim times within one day still read. The range is then
widened outward to whole units of the ticks, the coarsest unit it holds at
least twice, so its ends fall on the calendar, an instant just past the
bulk stays on the axis, and the widening at most doubles the range. The
height follows from the range as it did from the whole span, under the
same cap. Fewer than a hundred instants leave none out, and a graph whose
claims have no timepoint with a position keeps the recorded axis in claim
time, so that switching modes moves nothing. Recorded time is not clipped.

An instant beyond the range is drawn at the end it lies beyond: both ends
of its span are brought within the axis, so a column that starts before the
axis is drawn from its foot with the part inside as it is, one wholly
before it is a flat disc at the foot, and the same at the top. A small
arrow in the axis's colour points out of the axis from each clipped end of
a column, seen from the side as an arrowhead and from above as a cross. At
an end that clips a visible node the axis has a break mark, a short stub of
its line crossed by two slashes, with the count of the instants beyond it
as a label, "16 earlier" at the foot or "2 later" at the top; a column's
start and end count as two. The marks are sized in screen pixels, since at
the timeline view's scale a world unit is little more than a pixel, and
the break mark is left out while the axis is seen end on. Clipped nodes
stay pickable and keep their labels. The side panel's time section gives
a clipped node's true dates, where they lie against the axis and how it is
drawn, and the tray lists every clipped node under "Beyond the axis", first,
since its list is short and the other runs to nearly forty claims, each row with
its dates and a click that focuses the node. The time window, the "now"
preset and the fits work on the clipped spans: in claim time the window
lies within the axis, so a handle stops at its ends, and a window that
reaches an end passes what is clipped there; a "now" beyond the axis
stands at its end and its label says so. Once drawn the claim axis still
never rescales. A disc recorded after its top, live or in a snapshot read
again, grows it upward at its own scale, and the nodes clipped at the old
top are drawn again on it; a timepoint beyond it is clipped and grows
nothing.

Measured on the development graph on 2026-10-21: 2,456 placed instants,
from 364 columns and 1,728 discs. The central 98% runs from 23 July to 20
October 2026, widened to whole months, 1 July to 1 November, 415 units
high, and "now" is on it. Eight claims lie beyond it, all before it, with
16 instants: two on the interval from 1372 to 1399, and intervals in 1860,
1914, 1991, 2022, March 2026 and May 2026. None lies after it.

## 5. Edges

**Membership becomes geometry.** A claim sits beside its filing topic (§7),
takes its colour or its volume from its metacontext, and its source
document and tags are filters. None of the 13,800 membership edges is ever
a line. This is also what makes the scene legible at all; drawing them is
the hodgepodge the user predicted.

**Claim-to-claim edges are lines on demand.** `supports`, `timelink`,
`assessed`, `related` and `similarity` are off by default and drawn when a
node is hovered or focused: every edge touching it, as a line from column to
column. Where along the time axis the line sits is the edge's own
`created_at`, in recorded-time mode, and in claim-time mode the midpoint of
the overlap of the two columns, or of the two discs. A toggle shows all of
them for the current filter, for the user who wants the hairball once.

**No ribbons.** A plane spanning two columns' overlap asserts that the
relation held throughout that period. An edge has a recorded time and, with
41 exceptions none of them claim-to-claim, no validity, so the graph makes
no such claim and the picture must not either. A line at one instant is what
the data says. Textured planes with converging lines from a long column onto
a short one were considered and rejected for the same reason and for their
cost.

**Succession and merge edges are always drawn, vertically.** When node A
was corrected by, followed by or merged into node B, A's column ends and B's
begins, and a vertical connector between them is what makes the pair read as
one claim with a history rather than two bars. `superseded_by` and
`temporally_followed_by` draw as a connector from the top of the old column
to the bottom of the new; `merged_into` draws from each source to the
survivor; `evidence_superseded` and `evidence_merged` run from a fact to the
inference that depended on it and are drawn as the same connector in a
second colour. Hidden nodes stay hidden: a connector to a filtered-out node
ends in a stub with a count.

Because the snapshot holds only active nodes, the old end of a succession
is usually absent from it. The connector then starts at a marker at the
edge's `created_at` and the side panel names the missing node from the
edge. A later slice may ask Epimemer for historical nodes; the snapshot does
not send them today.

What the build settled (slice 6, measured on the development graph on
2026-10-20): none of the 148 succession edges with a drawn end has its old
end in the snapshot, so every connector there is the marker case. The
marker stands at the foot of a short stub down from the new node rather
than at the edge's `created_at`, because that instant is the new node's own
for a correction, a world-change or a merge, and later, by up to a month,
for the evidence edges, whose inference was recorded before its fact was
retired: a marker there would sit on the node or above it. Between two
drawn nodes a connector runs down the new node's axis to the old node's
height and across to it, so that it stands as a vertical in the timeline
view. A stub toward a hidden node carries no count yet; the count waits for
the side panel. The lines fade with the focus as the discs do, so the
toggle's mesh and the connectors' are rebuilt on a change of focus as well
as of filter, and never on a change of hover.

## 6. The scene and the cameras

One scene. The topic map is the ground plane (x, y), time is up (z). The
three views the user asked for are three cameras on that scene:

| Preset | Camera | What it is |
|---|---|---|
| Graph | Straight down, orthographic | The graph view: columns collapse to discs on the topic map, successions collapse to one point, which is the current state of each claim. |
| Timeline | Side-on, orthographic, looking along y | The timeline view: every column as a bar against the time axis, the topic map folded to one dimension. |
| 3D | Free perspective camera | Fly, orbit, dolly, with the usual keys and a slow default speed. |

More presets, because the point of presets is that the user is never lost:
per topic (fit the view to that topic's region and its columns), per metacontext
(fit the view to that volume), "now" (the latest layer in recorded-time mode), and
**reset**, one key, which returns to the 3D preset framing the whole graph.
A preset is a camera transform and a projection, animated over a short
tween so the user keeps their bearings. Presets are stable because the
layout is (§7), so "the Petritype topic from the south-east" means the same
thing tomorrow.

## 7. Layout

The layout has to satisfy three things at once: it must be stable, so that
presets and spatial memory work; it must be incremental, so a new node never
moves an existing one; and it must mean something, so that neighbours are
related. The current dashboard's force layout per load gives the third and
neither of the first two.

**Topics make the map.** The topic nodes are few (415 here) and name what
the graph is about, so they are the anchors:

1. Fetch the topic embeddings (`GET /api/embeddings?node_type=topic`).
2. Project them to 2D. **PCA first**: it is a fitted linear map, which places
   a new point exactly where its content puts it without refitting, is
   small in Rust (a 384×384 covariance and its two leading eigenvectors, by
   power iteration with deflation, `projection.rs`), and is deterministic.
   UMAP gives tighter clusters and is left open (§16); it would be a later
   option behind the same interface. The projection is scaled by one
   factor, keeping its aspect, so that 95% of the topics stand within the
   radius the topic circle had, and the outliers beyond it.
3. Relax briefly (`relax.rs`): nudge overlapping topics apart, and on a graph
   with `subtopic_of` edges pull each child toward its parent. Each topic's
   room is 2.5 units, its disc and a margin, and a quarter of the reach of
   its claims; a topic clear of every other's room is not moved at all.
4. **Freeze.** Persist the topic coordinates, keyed by graph and topic id,
   together with the fitted projection (the mean, the two component
   vectors and the scale), the embedding model id, the claim spread and
   the map's outer radius (`topic_map.rs`, `persist.rs`).

**Claims are placed, not laid out.** A claim's position is a pure function
of its filing topic's position and its own id: an angle from a hash of the
node id and a radius from how many claims that topic holds, so claims on a
busy topic spread wider. On the map the spacing between ranks is scaled by
the map's claim spread, below. Deterministic, so claims need no persistence and a
new claim never moves an old one; the time axis separates claims on one
topic anyway, so the within-topic spread only matters in the graph preset,
which is exactly where the hash jitter does its work. A claim tagged with
several topics is placed by its filing topic and the other tags are a
filter and a hover highlight.

**New topics** are projected through the persisted projection into the
existing map and nudged into free space: where the projection puts them if
that is clear of every topic's room, else the nearest clear point on a
spiral out from there. They do not refit the projection and they do not
move anyone.

**Recalculate** is a button, and `U`. It refits the projection over the
current topics, re-runs the relaxation and overwrites the persisted map.
The user presses it when enough topics have arrived that the map has
stopped making sense, and never otherwise; the bar suggests it once the
topics put on the map since the fit pass a quarter of those fitted. It
also runs itself, with a notice, when the persisted model id differs from
the one the embeddings route reports, because a changed embedder changes
every vector.

**Persistence is on the client.** Natively, a small file per graph under
the platform's data directory; in wasm, local storage. The server never
learns about pixels. Because claim placement is deterministic from that
file, two people with the same file see the same map, and the file is small
enough to share (§16).

**Why not embeddings for every node directly.** A 2D projection of 2,000
sentence embeddings smears into a cloud, loses the containment structure the
filing edges give for free, and shifts under every refit. Using them for the
415 anchors keeps the semantic neighbourhoods and discards those problems.

What the build settled (slice 2, measured on the development graph on
2026-10-21, 433 topics and 2,112 nodes). The route answered 433 topic
vectors under `all-MiniLM-L6-v2`, 384 long, 3.5 MB of JSON, read in about
400 ms and parsed in 7; the fit, the relaxation and the claim spread took
33 ms together in the debug build. The two components explain 10% of the
variance, and the third eigenvalue is 0.85 of the second, so the second
component converges slowly; the map still reads as
neighbourhoods: `visualisation` stands among `epimemer-bevy`, the Petri-net
visualiser and `petritype`; `tags` among `topic-merge` and the
tag-enrichment incident; `similarity` among `evidence-merged`, `verdicts`
and the seven-verdict taxonomy; `temporal-validity` beside the
ValidityInterval type and `provenance-and-accountability`; and the
dev-session topics gather along one side. The relaxation settled in 93
steps, moving the median topic 1.2 units and none more than 8.5, and
leaving 137 exactly where the projection put them; the map's outer radius, claims included, is 312 against the circle's 207,
since the outliers stand beyond the bulk.

At the full spacing of slice 1 a busy topic's claims reached further than
its neighbours stand: the median distance between nearest topics on the
relaxed map is 8.2 units, and the busiest topic, with 112 claims, reached
20, so its claims swallowed the topics around it. The map therefore keeps
a claim spread, the factor the median nearest-neighbour distance gives
against the 16 units between the circle's rings, which were tuned to keep
claims apart, between a quarter and one: 0.51 here. Only the spacing
between ranks scales, so the first claim keeps its clearance from its
topic's disc, and the busiest topic's claims now reach 12.6.

The map is kept per hub and graph: natively in
`<data directory>/epimemer-bevy/<host:port, encoded>/<graph>.json`, 40 KB
for the development graph, and in a browser in local storage under
`epimemer-bevy/<host:port>/<graph>`. On a start with a saved map the topics
the map names are placed there before the window opens, the rest on a
ring outside it, and the embeddings are still read, for the topics the map
does not know and the model's name; with none the circle is drawn, and the
topics and their claims move to the fitted map over a second and a half,
after which the view last asked for is fitted again. Two runs against the
same saved map gave byte-identical dumps of every topic's place, and a
recalculation over the same topics gave the same map and the same file,
since every step is deterministic. A topic stored live takes a slot on a
ring 16 units inside the ring of slots, and the embeddings are read two
seconds after the last new topic of a burst, because a topic's embedding is
stored after its `node_stored` event; one with no embedding stays on its
ring and the side panel says so. A topic the map names that stands
elsewhere, one back in the active set, say, is moved to its place. A hub
without the route, which answers `404` from its router rather than the
contract's `{"error"}` body, leaves the circle or the saved map drawn and
says why in the bar; a `404` with that body means the session is gone, and
the read chooses a session again by graph.

## 8. Rendering

- **Columns are instances.** One mesh, one material, per-instance colour,
  alpha and height. Bevy batches entities that share a mesh and a material
  handle; two thousand entities each with their own `StandardMaterial`
  would break that into two thousand draws, so per-instance data rides in
  Bevy 0.19's per-instance value, the `MeshTag`. Every disc shares one
  cylinder mesh and one `DiscMaterial`; its tag packs its kind's colour as
  sRGB bytes and its alpha as a fourth, RGBA8 with red lowest, which the
  material's shader unpacks, so every disc is drawn in one call whatever
  its colour or alpha.
- **Additive blending.** Thousands of translucent columns under ordinary
  alpha blending need back-to-front sorting and still show artifacts where
  they interpenetrate. Additive blending is order-independent, and where
  smears overlap the result is brighter, which is the density map the user
  wanted for free. Bevy's `AlphaMode::Add` on a standard material does
  this; a custom material does the same in its pipeline descriptor. A dark
  background is a consequence: additive on white washes out.
- **Translucency everywhere, not only on smears.** Column alpha is a
  function of distance from focus and of filter state, so that a filter
  dims rather than deletes and the user keeps the shape of the whole.
- **Picking.** Bevy's built-in picking, with the mesh picking backend added
  to the app, for hover and click on columns and discs. Hover highlights the
  node and its claim-to-claim edges; click focuses, which drives labels (§9),
  edges (§5) and the side panel (§10).
- **Per-update cost is O(changes).** Instance data is rebuilt only when the
  filter, the focus or the data changes, never on every update.

## 9. Labels

A knowledge graph is mostly text, and text is where a 3D view usually dies.
The rules:

- **A budget, not a radius.** On each update in which focus, camera or filter
  changed, score every visible node: distance to the focus (or to the
  camera's look-at point when nothing is focused), degree, node type (topics
  before inferences before facts) and importance where the node carries one.
  Take the top N, where N comes from the viewport area (roughly one label
  per 40,000 square pixels, tuned once), and walk down the ranking rejecting
  any label whose screen rectangle overlaps one already accepted. Fade by
  rank, so the last accepted labels are faint. The sphere the user described
  emerges from this: a dense neighbourhood fills the budget close in, a
  sparse one reaches further, with no radius parameter. Two things the
  build settled: a label may stand to the right of its disc, to its left,
  above or below, and takes the first place that overlaps no accepted
  label, because on the development graph the topics of one session stand
  within a label's height of each other in the whole-graph views and a
  single place left the focus's own neighbourhood almost bare; and a node
  the filter dims is labelled after every node that passes.
- **No 3D text.** The accepted nodes are projected to the viewport
  (`Camera::world_to_viewport`) and the labels are Bevy UI text nodes placed
  at those positions, in a layer under the panels. A hundred UI text
  entities is nothing, and it sidesteps every problem of text in a 3D scene:
  billboarding, depth, resolution, fonts.
- **Short on the label, full in the panel.** A label is the topic's name or
  the first forty characters of a claim; the side panel shows the whole
  content, description, status, confidence, source and times on focus.
- **Topic labels are sticky.** Topic names are the map's place names and
  keep a reserved share of the budget, so the user can always read where
  they are.

## 10. Filters and the interface

All Bevy UI, a bar along the top and a panel on the right, both collapsible.
The set is fixed; a filter not on this list is a design change:

| Control | What it does |
|---|---|
| Session and graph | Pick from `/api/sessions` and `/api/graphs`, as the dashboard does. |
| Time mode | Recorded time or claim time (§4). |
| Metacontext | Show one, several or all; each a colour or a separate volume. |
| Node kind | Topics, facts, inferences. |
| Status | Active only today; prepared for historical when the contract sends them. |
| Topic subtree | A topic and everything filed under it (and its subtopics where `subtopic_of` exists). A topic tagged with it counts too, one level deep, with the claims filed under that topic. |
| Time window | Two handles on the axis; outside dims, inside is full. |
| Source document | One `sourced_from` target; everything read from one document. |
| Retrieval | One retrieval record of the graph drawn (§3): the nodes its `retrieved` names pass. A record that declares nothing passes every node; one that declares an empty list passes none. |
| Edge toggle | Claim-to-claim edges for the whole filter, on or off (§5). |
| Search | Substring over content and description; hits are focused in turn. |
| Presets | The cameras of §6. |
| Recalculate layout | §7. |
| Side panel | The focused node in full, its edges by class, its timepoints, its validity brackets, and the connectors' far ends by name. |
| Log | The session's acts, advisories and pipeline runs, newest first, filtered by kind, in place of the side panel's node (slice 13). |

Filtering dims rather than removes by default (§8), with a "hide" variant
for when the user wants the filtered-out nodes gone from the budget and the
picking too.

Where the retrieval records come from (slice 12). `GET /api/retrievals`
is read for the session drawn when the first graph is drawn and again after
every switch of graph or session, because the stream follows only the
graph drawn and the route answers for every graph. Each
`retrieval_recorded` the stream brings, the replay's twenty among them, is
added as it comes, even while a graph is being read, since a record changes
nothing drawn. The two paths bring the same records on purpose, so a record
is kept once, by its `record_id`: the copy held first stays, except that a
copy with its query and response text replaces one without, which is what
the stream sends on a hub bound beyond loopback. The client keeps the 50
most recent records of the session followed, ordered by `record_id`, whose
text order is its numeric order, and forgets them on a switch to another
session, since ids are counted per session.

What the build settled for the retrieval focus (slice 12, 2026-11-07). The
picker lists only the records whose `graph` is the graph drawn, since ids
from another graph match nothing, newest first. Each row is the tool
without its `epimemer.` prefix, the time the tool answered in UTC, and how
many distinct nodes it retrieved, or "not declared"; a first row, "None",
clears the choice, as the Clear button beside the picker does. Choosing a
record copies its ids into the filter, so the choice holds after the record
falls out of the 50 kept; the picker's button names the record by its tool
and time while it is kept, and by its number after. A switch of graph
drops the choice, as it drops a topic, and a read of the graph again keeps
it. With a record chosen, the side panel's first line counts the record's
ids that name no active node of the graph drawn, which is the snapshot as
the stream has kept it, or says that the record declares nothing.
`--retrieval latest` or `--retrieval ID` chooses a record from the start,
once the route has answered, and `--ui-state retrievals` opens the picker.
Checked on the development graph on 2026-11-07, 2,442 nodes: the route
answered in under 30 ms and the picker listed its 20 records. Choosing the
search that named 13 nodes dimmed the other 2,429, with none of the 13
absent, at 62 to 64 frames a second as before; a `segment` record, which
declares nothing, changed nothing shown; and a `store_decomposition` that
declared an empty list hid every node in Hide mode.

The log (slice 13). `J`, or the bar's Log button, shows the log in
place of the side panel's node, at the panel's width, so the scene keeps
the share of the window it has beside the panel; `J` or Log again returns
to the node, `P` hides the panel either way, and `J` with the panel hidden
shows it on the log. `L`, for log, is the edge
toggle, so the key is `J`, for journal. The log lists the acts and
advisories of the graph drawn and the pipeline lines, which name no
graph, newest first: an act or an advisory by its `action_id`, the
position the contract gives, and a pipeline line after the act or advisory
held last when it came, in the order the lines came. An entry is one row:
its time in UTC, under a heading for each day; its kind, which is an act's
verb, an advisory's kind or a pipeline's name; and its words, an act's
summary, an advisory's tool and message, or the pipeline line. A fainter
row under it gives an act's counts and an advisory's action and whether
the agent was shown it, each as the string the hub sent, and who judged.
The subjects follow as the panel's node references, at most twelve, and a
click on one focuses that node and leaves the log open; a subject the
graph drawn does not hold is named by its short id and cannot be clicked.
An advisory with `notify_user` set says so in red after its time, on a
tinted row.
Five buttons above the list filter it: All, Acts, Advisories, Pipelines,
and Notify user, the advisories with `notify_user` set. The client keeps
512 acts and advisories, as many as the hub's ring holds, and 512 pipeline
lines, counted apart so that a busy pipeline cannot push the acts out, and
the log lists the newest 200 that pass, as the panel lists 200 edges. An
entry is kept once by its `action_id`, for the session followed across
switches of graph, and the log is emptied on a switch to another session,
since ids are counted per session. `--ui-state log` opens it from the
start.

The reflect counter (slice 13). The bar shows `reflect N/M`: the stores
since the last reflect against the threshold at which reflect is
suggested, followed by "suggested", in the accent colour, while the hub
says it is. The graph list's `reflect` describes the session's active
graph, so each graph list of the session drawn that comes, on connect, on
a switch, on a read again and when the graph picker opens, sets the
counter of that graph, and `reflect_counter_updated` sets the counter of
the graph it names. The bar shows the counter while it is the graph
drawn's, and `reflect -` for a graph the session is not on, whose counter
the hub does not report.

What the build settled (slice 9, checked on the development graph on
2026-10-20). The panel is 380 logical pixels wide with 8 around it,
nearly a third of the default 1280 by 720 window, so every view is fitted, and
every label placed, to its left while it is open, as they are below the
bar; collapsed, it gives the width back. Its sections follow the order of
the table above: the node's fields, its edges by class (membership, claim
to claim, succession and merge), then its time, which holds what the
tray's line for the focused node used to, so the tray itself is a section
of the panel in claim time and the bottom-right box is gone. The client
now keeps every node's whole NodeView, which the event stream updates in
place, and the panel takes it apart field by field, so a field added to
the contract does not build until the panel shows it. On a fact placed by
its tag, with two tags, a metacontext, a source, two claim edges and a
succession, the fields and every edge section fit the default window and
the time section is a scroll away. The old end of a succession, which
the snapshot holds for none of the 148 with a drawn end, is named by its
id from the edge, and the count of far ends the filter hides, which slice
6 left to the panel, is in each section's heading. The search field takes the
keyboard when clicked or on `/` and keeps it until Escape or a click
elsewhere, unlike the topic field, which follows the pointer, so that
Enter can walk the hits; "corroboration" finds 51 of the 2,097 nodes.
The focus history keeps 32 nodes by id, so it survives a read of the
graph again. `claim_kind` is not in NodeView (§16), so the panel cannot
show it.

## 11. Live updates

The client fetches the snapshot once and then applies the `/ws` stream:

| Message | Effect |
|---|---|
| `node_stored` | Place the node (§7) and add its column; update in place if the id is known. |
| `edge_stored` | Index by class; a succession connector appears at once, a claim-to-claim edge waits for hover. |
| `node_status_changed` | Remove the node from the active set and leave its succession marker (§5). |
| `timeline_stored` | Replace the timeline; recompute the claim-time columns that reference it. |
| `graph_switched` | Nothing by itself; the session's `active_graph` changes and the client shows whichever graph it was asked to. |
| `retrieval_recorded` | Keep the record for the Retrieval picker (§10), once by its `record_id`; nothing drawn changes. |
| `graph_action_recorded`, `advisory_raised` | Keep the entry for the log (§10), once by its `action_id`; nothing drawn changes. |
| `reflect_counter_updated` | Set the bar's reflect counter of the graph it names (§10). |
| The six pipeline events | A line in the log, or a duration added to one (below). |
| `session_connected`, `session_disconnected`, `session_dropped` | Update the session picker; a `session_connected` for the followed session refetches the snapshot, because events during the session's disconnection were dropped. |
| A `seq` gap, or the client's own reconnect | Refetch the snapshot. |

Replay on subscribe carries only acts, advisories and retrieval records.
The records fill the Retrieval picker and the acts and advisories the log
(§10). Like the records, the acts, the advisories, the reflect counter and
the pipeline events are taken as they come, even while a graph is being
read, since none of them changes anything drawn, and a body among them
that does not read is logged and dropped without reading the graph again.

The log makes its pipeline lines from the events until the activity bar
draws the runs (§13). `pipeline_started` is a line naming how many
transitions and places its topology has; `transition_fired` is a line
naming the transition, and the `transition_completed` that follows adds
its duration to the oldest such line still without one, or makes a line
of its own where the fire came before the client did; `pipeline_completed`
and `pipeline_failed` are a line with how many transitions fired and how
long the run took, and a failure names the transition and the error.
`tokens_updated` makes no line. The events carry no run id, so a run
already underway when the client connects shows from its next event,
since the topology is not replayed, and two runs of one pipeline at once
interleave their lines. The topology is parsed and kept on the event for
the activity bar, and the log keeps only its counts.

What the build settled (slice 8, 2026-10-20). The client subscribes before
it reads the snapshot, so an event between the two waits in the stream's
channel and is applied over the snapshot; applying an event the snapshot
already holds changes nothing, which is also what makes holding events
during a read safe. Epimemer 0.2.11's hub, older than the contract,
answered a subscribe for the development graph with its replay, three
acts and an advisory and then seven retrieval records, `seq` 1 to 11, in
the contract's shapes, and the socket stayed open through its pings. Each
update drains the stream: a stand-in hub's replay of one decomposition and
one correction onto a copy of the development graph, 97 events, was
applied in one update in 2.6 ms, and a test's burst of 441 events onto a
graph of 2,020 nodes in under a millisecond; the filter's pass over every
node runs once per such update, as on any change of filter. A claim is
drawn in the unfiled region until its filing edge, which is an event of
its own, arrives; within an update the move is never seen. A tag that
comes first moves it to the tag's topic, and the filing edge then moves it
once more and for good. The step past the circle's last topic comes back
round to the first, so a new topic keeps the circle's step on a ring 16
units inside it, then further in, and finally outside the unfiled region
(`layout.rs`); with the embedding map of §7 the ring stands outside the
map, and the topic waits on it until its embedding is read. Both time axes grow
upward at their own scale, the claim axis past its cap where it must, and
an instant below an axis's start stands below 0. A node that leaves the
active set is hidden whatever the filter, and its edges keep it as an id,
so its connector ends in a marker. A read of the graph again, after a gap,
a reconnect, or the followed session's return from a disconnection, is
seeded with the picture drawn: killing and restarting a stand-in hub under
a running client read the graph again with all 2,111 nodes where they had
been. A `session_connected` for the followed session reads the graph again
only after a disconnection was seen, since one that only changes the active
graph follows no lost events. Three things the stream cannot say: a timeline
decision re-sends the timelinks it retires as `edge_stored`, and the view
carries no `retired_at` to tell them from live ones, so a retired timelink
stays until the next read; an edge the server deletes sends nothing; and a
node never drawn that comes back into the active set has no view to draw,
so it asks for the graph to be read again.

## 12. What Epimemer provides, and does not

Provided, in `VIZ_CONTRACT.md`: the schema version, the snapshot, the event
stream, and the embeddings route with its `node_type` filter, added for this
client. Two promises followed on 2026-11-07 for the views coming from the
dashboard, both additive, so the schema stays 1:

- the retrieval record's fields: `record_id`, `at`, `tool`, `query`,
  `graph`, `retrieved` as a list of `{node_id, provenance, score}` or null
  for "not declared", and `truncated`;
- the replay rule: the last 512 acts and advisories interleaved, then the
  last 20 retrieval records; pipeline events, `pipeline_started` and its
  topology among them, are never replayed.

The hub is to serve this client's browser bundle from a directory that
`epimemer viz fetch` fills with a pinned, checksummed release of this
repository; that Epimemer change is not made yet.

The contract names what is not promised: positions, as-of snapshots,
embeddings for segments, the contents of `metadata`, and `claim_kind` on the
wire (§16).

Not asked for, deliberately: a layout endpoint (the layout is the client's,
§7), historical nodes in the snapshot (wanted for §5's connectors, but a
second read on every snapshot and a later request once the first slices
show how often it matters), and CORS headers (needed only by the wasm build
served from another origin, §13).

## 13. Later work

**The activity bar.** A strip along the bottom showing Epimemer's Petri net
pipelines (segment, store, reflect and the rest) as small live glyphs lit by
`transition_fired` and the other pipeline events, the way the dashboard's
pipeline strip does, and the thing the user is drafting as an "activity
thumbnail" in `petritype-bevy`. This is where `petritype-bevy` enters, as a
crate dependency, and where its prerequisites bind: an input that mirrors a
net the app does not run, driven by an external event stream rather than an
in-process `Net` (its remote source is halfway there), a wasm target, and
`petritype-rs` and `petritype-bevy` published to crates.io so that anyone
but the author can build this crate. Path dependencies are acceptable while
incubating locally, exactly as `petritype-bevy` depends on `petritype-rs`
today.

**The wasm target.** The same code in the browser, with costs to budget for:
a Bevy wasm bundle of 10 to 30 MB before compression and perhaps 5 to 8
after `wasm-opt` and brotli; no threads unless the page is served with
COOP and COEP headers, so the layout and the projection run on the main
thread or in a worker by hand; WebGL2 as the fallback where WebGPU is
absent; and serving, either a static-directory option on the hub (which
also solves CORS, since the page is then same-origin) or a separate static
server with CORS headers added to the hub. Local storage replaces the layout
file.

**What slice 10 settled.** One crate builds for both targets. The hub's reads
and the event stream are futures written once over a transport each
platform implements: natively ureq and tungstenite, blocking, on a thread
each, as before; in a browser the page's fetch and WebSocket, as tasks on
its event loop. Their answers come back on the same channels, which the
systems drain without knowing the platform. Nothing in the browser needs a
thread, so the page needs no COOP and COEP headers, and the first graph is
placed on the page's own thread: for the development graph's 2,112 nodes
that takes a few milliseconds once the 6.2 MB snapshot has arrived. The
bundle is 28.6 MB after `wasm-opt -Oz`, 9.2 MB with gzip and 6.0 MB with
brotli, within the estimate above, once a release profile optimises the
whole program for size; without that profile it is 45 MB. WebGL2 is the
only backend: Bevy's `webgpu` feature replaces WebGL2 rather than falling
back to it, so WebGPU would be a second bundle, and nothing the scene draws
needs it. `trunk serve` is the development server and passes `/api` and
`/ws` on to the hub, so no CORS is needed; the deployment is the hub
serving the bundle from a static directory, an Epimemer change not made
yet. Slice 2 keeps the topic map in local storage there (§7).

**As-of scrubbing.** If Epimemer grows an as-of snapshot route, a scrubber
on the time axis shows the graph as it was, and the recorded-time mode
becomes a replay. Nothing in the layout changes for it.

**Historical nodes.** See §5 and §12.

**The valid-time grammar.** Epimemer's timeline-panel page sets it out in
its §13, with a mockup (`dev-docs/TIMELINE_VISUALISATION.md`). Both move
here when the dashboard is deleted, as a reference for how validity is
drawn. Nothing in this design changes for it.

**Native binaries.** Native builds as release assets are later work, outside
the migration; slice 14 releases the browser bundle alone.

## 14. Order of work

Slices, each with tests, each approved before it starts. The first slice is
deliberately small: it is the test of whether the picture is legible at all.

1. **Columns.** A native binary that connects to the hub, lists sessions and
   graphs, fetches the development graph's snapshot, places topics on a
   circle (no embeddings yet) and claims around them by the hash rule, and
   draws every node as a disc at its recorded time with a free camera and
   additive blending. Nothing else: no labels, no edges, no filters.
   Acceptance: the 2,026 nodes of the development graph render at 60 fps
   on the author's laptop, the camera flies, and the picture is recognisably
   the graph growing over time.
2. **The topic map.** The embeddings route, PCA, relaxation, persistence,
   new-topic placement, and the Recalculate button. Acceptance: closing and
   reopening the app shows the same map; adding a topic through the MCP
   server places it without moving any other; Recalculate produces a new map
   and persists it.
3. **Presets.** Graph, timeline, 3D, per-topic, per-metacontext, now, reset,
   with tweened transitions. Acceptance: each key lands the camera where
   this document says, and the graph preset is readable as the dashboard's
   graph view is.
4. **Filters.** The bar of §10 except search and the edge toggle, with dim
   and hide. Acceptance: every filter in the table acts as described on the
   development graph and the fps holds.
5. **Labels.** The budget, overlap rejection, fade, UI overlay, sticky topic
   names. Acceptance: at every preset the labels are readable, none
   overlap, and the budget visibly follows density around the focus.
6. **Edges.** Hover and focus lines for claim-to-claim edges, the edge
   toggle, and succession connectors always on, with markers for absent old
   ends. Acceptance: focusing an inference shows its supporting facts as
   lines; the 192 succession edges on the development graph are visible as
   verticals; the toggle shows and hides the rest.
7. **Claim time.** The second time mode, the column vocabulary of §4
   including smears, the tray, and validity brackets on focus. Acceptance:
   the 55 interval points draw as solid columns, the 29 vague points as
   smears with the right bounds, and the 1,214 unlinked claims stay discs.
8. **Live updates.** The `/ws` client, incremental application, snapshot
   refetch on gap and reconnect. Acceptance: a `store_decomposition` on the
   MCP server appears in the scene within a second without a reload, and
   killing and restarting the hub recovers without one.
9. **Panel and search.** The side panel in full and the search box.
   Acceptance: every field of the contract's `NodeView` is visible for a
   focused node, and search walks hits.
10. **Wasm.** The browser build and its serving story (§13).
11. **The activity bar** (§13), once `petritype-bevy` can mirror an external
    net and both Petritype crates are published.
12. **Retrieval focus.** Parse `retrieval_recorded` from the stream, and read
    `GET /api/retrievals` on connect and on a switch of graph. A Retrieval
    picker in the bar, beside the other pickers, lists the records newest
    first, each with its tool, its time and how many nodes it retrieved, and
    adds a filter criterion: a node passes when the chosen record's
    `retrieved` list names it. A record whose `retrieved` is null, which
    declares nothing, passes every node; a declared empty list passes none.
    With a record chosen, the side panel says how many of its retrieved ids
    are absent from the snapshot. Dim and hide work as for every filter.
    Acceptance: on the development graph, choosing a record dims or hides
    every node it did not retrieve and the fps holds; a record that declares
    nothing changes nothing; clearing the picker restores the view;
    `just check` passes.
13. **Log view.** Acts (`graph_action_recorded`) and advisories
    (`advisory_raised`), from the replay and the stream, in one list, newest
    first, with a filter by kind; an advisory with `notify_user` set is
    marked, and a click on a subject id focuses that node. An advisory's
    kind and action, and the keys of an act's counts, are shown as the
    strings they arrive as, since the contract leaves them open. The bar
    shows the reflect counter, from the graph list's `reflect` fields and
    then `reflect_counter_updated`. Until slice 11 draws pipeline runs, the
    log shows them as lines from the pipeline events: a run's start, each
    transition, and its end, with durations; a run already underway when
    the client connects shows from its next event, since the topology is
    not replayed. A toggle in the bar opens the log in place of the side
    panel or beside it; the builder decides which and records it in §10.
    Acceptance: the replay fills the log on connect; a store made through
    the MCP server appears as an act, with its advisories, in the same
    update; a reflect run appears as pipeline lines; `just check` passes.
14. **Release.** A GitHub Actions workflow on a `v*` tag runs the gates of
    `just check`, builds with `trunk build --release`, packs `dist/` as
    `epimemer-bevy-web-<tag>.tar.gz` with a `.sha256` beside it, and attaches
    both to a GitHub release of the tag. The version in `Cargo.toml` becomes
    0.2.0. The README gains a paragraph on deployment: the hub serves the
    unpacked bundle from a directory, which `epimemer viz fetch` fills.
    Acceptance: a tag produces a release whose tarball, unpacked into a
    directory an Epimemer hub serves, loads the development graph in a
    browser with no console errors, and whose checksum verifies.
15. **Abstraction.** Level of detail by zoom, for large graphs: a busy
    topic's claims collapse into one glyph carrying their count until the
    camera is close enough, edges are thinned to the detail shown, and the
    label budget (§9) is spent on what is drawn. The glyph, the thresholds
    and what close enough means are written into §7 and §8 before the slice
    starts, as for every slice. Acceptance: a graph of two thousand nodes is
    readable at the overview in the browser build, the fps holds, zooming
    in reveals the claims without moving anything, and `just check` passes.

## 15. Risks

- **Text.** The budget (§9) is the mitigation, and slice 5 is where the
  design is proven or revised. If forty labels are not enough to navigate
  by, the fallback is a 2D minimap of topic names in a corner.
- **Transparency.** Additive blending removes sorting artifacts but ties the
  design to a dark background and makes very dense regions saturate to
  white. A per-instance alpha cap by local density is the first remedy.
- **Wasm size and threads.** Known costs (§13), deferred to slice 10, which
  met them: 28.6 MB after `wasm-opt -Oz`, 6.0 MB with brotli, and no threads.
- **Layout drift.** Many new topics placed by projection and nudging without
  a recalculation leave the map locally crowded. The Recalculate button is
  the user's remedy; a notice when the nudged fraction passes a threshold
  tells them when.
- **The embedder changes.** Every vector changes and the map must be
  rebuilt, which loses spatial memory once. The model id in the persisted
  file detects it and the notice says why.
- **The old end of a succession is not in the snapshot.** Markers (§5)
  until Epimemer sends historical nodes.
- **Bevy churn.** Bevy's API changes roughly quarterly. The crate pins a
  version and upgrades on its own schedule, which is one of the reasons it
  is a separate repository.

## 16. Open questions

- **PCA or UMAP.** PCA first for the reasons in §7. If the map is too
  uniform to navigate, UMAP (or a cheaper neighbour-embedding) is the next
  candidate, behind the same projection interface, at the cost of inexact
  placement of new points.
- **Projection on the client or the server.** The client, as designed,
  because the server should not learn about pixels and the client needs the
  fitted projection to place new topics. If several clients should agree on
  a map without sharing a file, that is the argument for moving it.
- **Should the layout file be shareable.** It is small and deterministic
  placement makes it sufficient to reproduce a map, so a "copy layout" and
  "paste layout" pair is cheap. Whether anyone wants it is unknown.
- **`claim_kind` does not reach the client.** Checked on 2026-10-20: a
  fact's `claim_kind` is a field on the stored fact, `NodeView` has no field
  for it, and `node_to_view` does not copy it into `metadata` (the view's
  `metadata` is the node's own free dict). So the client cannot tell a state
  from an event today. Adding `claim_kind` to `NodeView` is an added field
  under the contract's versioning rule and would let the client draw a state
  as a column and an event as a disc even without a timepoint.
- **Historical nodes in the snapshot**, for the old ends of successions
  (§5).
- **Importance.** Whether the node carries an importance a label score can
  use beyond `confidence` and degree; the snapshot has `confidence` only.
