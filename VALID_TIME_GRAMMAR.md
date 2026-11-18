# Valid-time grammar

A reference for how validity is drawn: the per-source periods each
`sourced_from` edge carries in the snapshot and on the live `edge_stored`
event, which say when a source asserts a claim was true. It was written for
the timeline panel of Epimemer's TypeScript dashboard and moved here on
2026-11-18, when that dashboard was removed from Epimemer. This visualiser
does not draw it yet (DESIGN.md §13); a slice that draws validity starts from
this page.

`mockups/valid-time-grammar.html` is a self-contained rendering of every mark
below, in a light and a dark theme, and is the visual reference for the marks.
It draws them flat, on a vertical axis with lanes beside it.

**The governing principle: one mark per validity concept, and nothing drawn
that the data does not assert.** If two things are different in the model
they must be different on screen; if the model refuses to claim something
(open-world gaps, unknown endpoints), the pixels must refuse too.

## 1. The marks

| Concept | Mark |
|---|---|
| Endpoint: **point** (stated) | Crisp bar cap: a short perpendicular tick |
| Endpoint: **unknown** | The bar **dissolves**: a gradient to fully transparent over a short fixed length. "The edge is somewhere in this fog" |
| Endpoint: **unbounded** | The bar keeps **full weight and runs off the edge of the drawing**. "There is no edge" |
| **Witness point** | Solid dot on the bar with a faint halo ring. A claim with no endpoints and one witness, the commonest real record, is a dot with the bar fading **symmetrically** away from it in both directions |
| **Vague label, resolved** ("during the Renaissance" with explicit approximate bounds) | **Hatched** band, the stored label rendered verbatim on the band |
| **Vague label, unresolved**, nothing else placing the interval | Never placed on the axis. Goes to a tray beside the axis as a chip, label intact |
| **Labelled edge**: a named endpoint on an interval whose other endpoint or witness has a date | The placed part draws as usual. Towards the named side the bar keeps full weight for a short stub, the length of a fade, and ends square with the stored label sitting at its end, verbatim. No tick, no date, no fade: the edge is a word, and the word is drawn where the edge would be |
| **Per-source validity** | One thin strip per source, stacked beside the fact, each **direct-labelled** with its source. No filled union bar, ever. An optional summary is a **hollow dashed envelope** that reads "any source asserts", outline-only so it cannot be read as a claim |
| Interval marked **stated** | Solid fill |
| Interval marked **inferred** | Hollow fill (soft tint) with a dashed outline; squinting performs the stated-only filter the API offers |
| Reflect-**proposed** boundary: an unknown edge reflect has offered a date for, taken from the claim on the other side of a succession | The solid body stops where the record's own dates stop. From there a hollow dashed extension in the pending colour runs to the proposed date and ends in a dashed cap of the same colour, marked "proposed · review". The extension replaces the fade that edge would otherwise wear, so an endpoint still carries one mark. Accepting rewrites the interval with that date and `inferred` basis, so the same stretch turns into an ordinary hollow dashed bar in the claim's colour |
| **Several periods** (one claim, one source, more than one interval) | One bar per period in the source's lane, joined by a **hairline dotted spine**; the spine says "same claim, nothing asserted here" |
| A recurrence **rule's occurrences** | Beads on a hairline dotted spine, one lane per rule; the spine says "same rule, nothing asserted in between". An occurrence that was made a real timepoint gets no bead, since it is drawn as the ordinary point it became, and a moved occurrence's bead sits at the date it moved to |
| A **point the order places**: an undated timepoint a source put between two dated ones | The hatched band, spanning its derived `earliest` and `latest`, with its label on it word for word. With one bound known the band runs a short way past it and dissolves |
| **Disputed order** | The point keeps whatever place it had and gains a zigzag in the contradiction colour beside it. The stroke runs along the axis and doubles back, so what reads as tangled is the sequence; a cross or a ring over the mark would put the doubt on the date, which a dispute about order leaves standing |
| **`temporally_followed_by`** | A small elbow connector from the end of one claim's bar to the start of the next, with a terminal dot: order, not replacement |
| Status **`HISTORICAL`** | Fainter bar, **never hidden**: still true of its period |
| Status **`CORRECTED`** | Hidden by default, mirroring `include_corrected=off`; struck through when asked for |
| The **now-line** | A dashed **neutral** rule across the axis, labelled "now". It is the timeline's `reference_time`, or the wall clock where none is stated |

## 2. Rules: places a reasonable rendering would lie

1. **A gap is never styled as "false".** Open world: outside a stated
   interval is *no assertion*. A red or shaded gap draws a claim nobody made.
2. **Unknown and unbounded never share a treatment.** They are different
   values in the model, so fade for one, running off the edge for the other.
3. **A label-only interval never sits on the axis.** It waits in the tray
   until resolved; resolution *adds* a position, it never replaces the words.
   An interval with a date at one end and a label at the other is not
   label-only: the date goes where dates go and the label is drawn as a
   labelled edge, never as a fade, since a fade says "unknown" about an edge
   the source named.
4. **No filled union bar.** "No default collapse" is a data rule; the hollow
   envelope is the only summary allowed.
5. **Historical is muted, not hidden; corrected is hidden, not deleted.**
6. **Cross-clock claims never share an axis.** Comparison across timelines is
   `unknown` by definition; an in-universe claim on the CE axis asserts a
   mapping nobody made. Tray, with a clock badge.
7. **Bars fade *through* the now-line, they do not stop at it.** Stopping
   asserts an endpoint. Currency is a reading against the timeline's
   reference clock, not a stored boundary.
8. **Soundness flags sit on the inference, not on its premises.** The
   premises are not what is unsound.
9. **A proposal never thickens the body.** The solid part ends at the last
   date the source gave. What reflect offers is drawn in the pending colour
   and hollow, the reading it would get once accepted, since an accepted
   boundary makes the interval `inferred`; until then it is not the record and
   is not coloured as if it were.

## 3. Notes for whoever draws it

- **Every `temporally_followed_by` walk must be cycle-safe.** Recurrence makes
  cycles legal for this edge. Each step is drawn once, and a walk stops on a
  step it has already drawn.
- **Detail on demand carries what the mark compresses**: the `(source,
  interval)` pairs verbatim, endpoint kinds by name, `stated` or `inferred`,
  and the witness date. The mark is the summary; the detail is the record.
- **Claim time only.** These are periods the world was in, so they belong on
  the axis that measures when things happened; plotting them against
  `created_at` would put a date on the wrong clock.
- **A lane's position carries no meaning**, so a lane is free again once the
  period in it has ended, and a drawing that runs out of lanes counts the
  rest rather than overlapping them.
- **A named endpoint keeps the rest of its interval on the axis.** An interval
  draws when a date or a witness places it, and each endpoint wears its own
  mark. The stub of a labelled edge asserts no extent: it is a fixed length on
  screen. Only an interval nothing places (two labels, or a label beside an
  unknown or unbounded edge with no witness) goes to the tray.
- **A proposed boundary is read, never worked out here.** The snapshot carries
  `boundary_proposals`, worked out by Epimemer from the same rule `reflect`
  uses, and leaves out the ones a judge declined. Accepting goes through
  Epimemer's `apply_reflection`, never through the visualiser. An interval
  whose only date would be the proposal stays in the tray, marked as pending.
- **A timeline with a stated `reference_time` keeps its own clock.** An
  interval naming no clock is measured against the wall clock, so it draws on
  a timeline that follows the wall clock and on no other. An interval naming
  this timeline always draws. Anything else is cross-clock and goes to the
  tray with the clock badge.
- **Source strips carry no meaning in their colour.** A strip is always
  labelled with its source, so its colour only has to differ from its
  neighbours', and must not be one of the colours that say what kind of node
  something is.
