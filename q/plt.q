/ plt.q: terminal plots for q.
/ https://github.com/jparmstrong/jplots   MIT
/ Everything here is argument munging: whatever you pass (a table, a keyed table, a dict, a
/ vector, a list of vectors, or a dict of `data` plus style) is normalised into ONE spec
/ dictionary and handed to `.plt.draw`, which renders it. Keeping the shape decisions in q is
/ what lets the API change without rebuilding the library.
/ (No bare "/" lines in this file: a lone slash opens a block comment to EOF.)

/ The renderer. A host that already provides `.plt.draw` keeps its own; anywhere else (kdb+)
/ the shared library is loaded. Testing RESOLUTION rather than `key .plt` matters: a
/ name-resolved builtin never appears in the namespace dictionary.
.plt.lib:$[count getenv`JPLOTS_LIB; `$":",getenv`JPLOTS_LIB; `:libjplots]
if[`nodraw ~ @[{.plt.draw};::;{`nodraw}];
  .plt.draw:.plt.lib 2:(`draw;1);
  .plt.info:.plt.lib 2:(`info;1)];

/ The renderer defaults to whichever one THIS terminal can draw. Sixel reaches the most
/ terminals and is the default, except in the two that implement the kitty protocol and not
/ sixel: kitty itself and ghostty. Both name themselves in the environment, so this costs no
/ terminal round trip at load time, unlike the `.plt.info[]` query further down. Assign after
/ loading to override.
/ `getenv` already gives a char vector, so `string` on it would split every character.
/ Ghostty is named by its own variable as well, whose VALUE is a path: what is being read
/ there is that the variable is set at all.
.plt.trm:lower (getenv`TERM),"|",(getenv`TERM_PROGRAM),$[count getenv`GHOSTTY_RESOURCES_DIR; "|ghostty"; ""]
.plt.rend:{[t] $[(t like "*kitty*") or t like "*ghostty*"; `kitty; `sixel]}
.plt.renderer:.plt.rend .plt.trm
.plt.theme:`dark;                              / `dark | `light
/ Where a sixel image leaves the cursor. The protocol does not say. Nearly every terminal
/ advances past the image and the default assumes so; on one that does not, every plot is drawn
/ over the last, and `advance` makes this side do the advancing instead. Setting it on a
/ terminal that DOES advance triples the gap under every plot, so it is not a safe default.
/ `jplots-probe --cursor` draws one frame per setting so a terminal can be asked by looking.
.plt.sixel_cursor:`reserve;                    / `reserve | `advance | `bare | `newline
.plt.bins:30;                                  / default histogram bin count
.plt.size:();                                  / (width;height) in pixels; () = fit the terminal
.plt.font:();                                  / integer glyph scale; () = match the terminal's text
.plt.scale:();                                 / multiply size AND font; () = 1 (HiDPI escape hatch)
/ `.plt.info[]` reports what the renderer detected about the terminal. The renderer cannot
/ read q globals, so the backend the NEXT plot will use is added here rather than there, which
/ also keeps the answer identical on every host. Guarded on `detect` rather than on calling
/ `.plt.info[]`: that call queries the terminal and waits for a reply, and loading this file
/ should not.
if[not `detect in key `.plt;
  .plt.detect:.plt.info;
  .plt.info:{[x] (.plt.detect x),(enlist `renderer)!enlist .plt.renderer}];

/ An axis format from the x column's type: temporals get calendar/clock ticks, symbols and
/ strings become categorical positions, everything else is numeric.
.plt.xf:{t:abs type x;
  $[t=14h;`date; t in 16 17 18 19h;`time; t=12h;`timestamp; t in 11 10h;`cat;
    (0<count x) and all 10h=type each x;`cat;   / a list of STRINGS is categorical too
    `num]}
/ Temporals reach the renderer in ONE unit per axis kind, milliseconds for a clock axis,
/ so a minute or a second column is scaled here rather than teaching the renderer four more
/ units. `15 xbar time.minute` is how q buckets, and it yields minutes.
.plt.xu:{t:abs type x; $[t=17h;60000f; t=18h;1000f; t=16h;1e-6; 1f]}
.plt.xv:{(.plt.xu x)*"f"$x}                    / an x column in the renderer's unit
/ Category labels from a column. `string` on a list of STRINGS splits each one into
/ one-character strings, and the renderer then reads no label at all, so strings pass through.
.plt.str:{$[10h=type x; x; string x]}
.plt.cats:{.plt.str each x}
/ A bar sits at a POSITION, not at a value: bars are drawn at 0, 1, 2 and so on whatever the
/ x column held. The axis therefore has to carry LABELS, and a bar chart is categorical no
/ matter what it was made from. Without this the renderer formats the INDEX in the column's
/ own type, and a chart of daily volume for 2026 came out with an axis reading 2000.01.01.
.plt.axf:{[k;f] $[k in `bar`hbar; `cat; f]}
/ A table: the FIRST column is x, every remaining column is a series named after itself.
/ A one-column table is that column against its row index.
.plt.of:{[k;t]
  t:0!t; d:flip t; c:cols t; m:1<count c;
  xc:$[m; first c; `]; yc:$[m; 1_c; c];
  f:.plt.axf[k; $[m; .plt.xf d xc; `num]];
  s:`kind`x`y`names`xfmt`xcats`xlabel!(k;
    $[not m; (); f=`cat; "f"$til count d xc; .plt.xv d xc];
    "f"$d yc; yc; f;
    $[f=`cat; .plt.cats d xc; ()];
    $[m; string xc; ""]);
  / One series draws no legend, so its column name would appear nowhere at all. Name the
  / y axis after it, the way the x axis is already named after the column it came from.
  / Several series keep the legend and an unlabelled axis; an explicit `ylabel` wins either way.
  $[1=count yc; s,(enlist `ylabel)!enlist string first yc; s] }
/ A plain dict (`exec v by k`): keys are x, values the single series.
.plt.od:{[k;d]
  f:.plt.axf[k; .plt.xf key d];
  `kind`x`y`xfmt`xcats!(k; $[f=`cat; "f"$til count d; .plt.xv key d]; enlist "f"$value d; f;
    $[f=`cat; .plt.cats key d; ()]) }
/ A vector (y against its index) or a list of vectors (several series).
.plt.ov:{[k;x] `kind`y!(k; "f"$$[0h=type x; x; enlist x]) }
/ The dict form: `data` carries whatever the plain form accepts, and the rest are per-plot
/ style, overriding the `.plt.*` globals for this call only. An unrecognised key SIGNALS:
/ a silently ignored `titel` is worse than a stopped query.
.plt.opts:`data`title`xlabel`ylabel`names`renderer`theme`size`font`scale`bins`overlay`fit_line`sixel_cursor`style`x
/ An options dict is symbol-keyed and has a `data` entry. A keyed table keys on a TABLE,
/ so `type key x` tells the two apart. The tests are nested `$[...]` rather than `and`
/ because q's `and` is `min`: it evaluates BOTH sides, and `key` of a simple table is 'type.
.plt.isopt:{$[99h<>type x; 0b; 11h<>abs type key x; 0b; `data in key x]}
/ The session settings, folded into every spec. The library cannot read q globals, so the
/ spec is the only channel. A host with its own globals is expected to check the spec first,
/ so merging here is correct either way. Only settings that are actually set are included.
.plt.settings:{[]
  v:`size`font`scale!(.plt.size;.plt.font;.plt.scale);
  (`renderer`theme`bins`sixel_cursor!(.plt.renderer;.plt.theme;.plt.bins;.plt.sixel_cursor)),
    (where 0<count each v)#v }
.plt.mk:{[k;x]
  o:()!();
  if[.plt.isopt x;
    if[count b:(key x) except .plt.opts;
      '"plt: unknown option ",(", " sv string b),
        " (known: ",(", " sv string .plt.opts),")"];
    o:((key x) except `data)#x; x:x`data];
  s:$[98h=type x;                     .plt.of[k;x];
      99h=type x; $[98h=type value x; .plt.of[k;0!x]; .plt.od[k;x]];
                                      .plt.ov[k;x]];
  if[`overlay in key o; s:.plt.ovl[s;o`overlay]; o:(key[o] except `overlay)#o];
  (s,.plt.settings[]),o }                      / per-call options win over the session
.plt.line:{.plt.draw .plt.mk[`line;x]}
.plt.scatter:{.plt.draw .plt.mk[`scatter;x]}
.plt.bar:{.plt.draw .plt.mk[`bar;x]}
.plt.hist:{.plt.draw .plt.mk[`hist;x]}
/ Horizontal bars: one row of the table per bar, top row first, its label on the left. Every
/ NON-numeric column is part of the label and every numeric one is a series, so a benchmark
/ table of query, runtime and elapsed reads as one bar per query/runtime pair without the
/ caller having to glue the two into a key first. The value axis is x, so the axis names the
/ table form derived are swapped before any per-call option is applied on top.
.plt.hl:{[t] d:flip t; c:cols t; v:c where (abs type each d c) in 5 6 7 8 9h; l:c except v;
  if[0=count v; '"plt.hbar: no numeric column"];
  k:$[count l; {" " sv .plt.cats x} each flip d l; string til count t];
  flip ((enlist $[count l; `$" " sv string l; `row])!enlist k),v#d }
.plt.hbar:{[x]
  o:()!(); if[.plt.isopt x; o:((key x) except `data)#x; x:x`data];
  / A table or a keyed table; a plain dict (`exec v by k`) is already labels against values.
  g:$[99h<>type x; (); 98h<>type key x; (); .plt.cats (key x) last keys x];
  x:$[98h=type x; .plt.hl x; 99h<>type x; x; 98h=type value x; .plt.hl 0!x; x];
  s:.plt.mk[`hbar;x];
  s:s,`xlabel`ylabel!.plt.at[s;;""] each `ylabel`xlabel;
  / A keyed table colours each bar by its LAST key: `by query, cache` gives cold one colour
  / and warm another, with a key naming them. One key colours every row in turn.
  if[count g; s[`groups]:g];
  .plt.draw s,o }
/ OHLC candlesticks. The renderer takes open/high/low/close POSITIONALLY, so the columns are
/ arranged here by name and a missing one is caught in q where the message can say which.
/ x is whatever column is left over: a time, a timestamp or a date.
.plt.ohlc:`open`high`low`close
.plt.candle:{[x]
  o:()!(); if[.plt.isopt x; o:((key x) except `data)#x; x:x`data];
  t:0!x;
  if[count m:.plt.ohlc except cols t; '"plt.candle: no ",(", " sv string m)," column"];
  xc:first (cols t) except .plt.ohlc;
  if[null xc; '"plt.candle: no column left for the x axis"];
  d:flip t; f:.plt.xf d xc;
  s:`kind`x`y`names`xfmt`xcats`xlabel!(`candle;
    $[f=`cat; "f"$til count d xc; .plt.xv d xc];
    "f"$d .plt.ohlc; .plt.ohlc; f;
    $[f=`cat; .plt.cats d xc; ()]; string xc);
  .plt.draw (s,.plt.settings[]),o }
/ An overlay: extra series drawn as LINES on top of whatever the plot's own kind is, which
/ is how a fitted model is shown against the sample it came from:
/    .plt.scatter ([data:sample; overlay:([] x:xs; fit:a+b*xs)])
/ It takes the same shapes as `data` and is named after its columns, so it gets its own key
/ entry and its own colour. Sharing one x vector would force the model onto the sample's
/ points, so each series is given its own. A model is usually drawn at two points, or at a
/ hundred, rarely at the sample's.
/ `s k` on an ABSENT key is a null and `count` of a null is 1, so every read here goes through
/ `.plt.at`: a missing `x` read directly becomes n nulls instead of "use the row index".
.plt.at:{[d;k;v] $[k in key d; d k; v]}
.plt.xper:{[x;n] $[0=count x; n#enlist "f"$(); 0h=type x; n#x; n#enlist x]}
.plt.ovl:{[s;o]
  v:.plt.mk[`line;o];                          / the overlay reads exactly like data
  n:count s`y; m:count v`y;
  nm:{[s;c] `$string .plt.at[s;`names;c#`]};
  s[`x]:(.plt.xper[.plt.at[s;`x;()];n]),.plt.xper[.plt.at[v;`x;()];m];
  s[`y]:(s`y),v`y;
  s[`names]:(nm[s;n]),nm[v;m];
  s[`overlay]:(n#0b),m#1b;
  s }
/ Least squares y on x, ready to hand to `overlay`. Two points are a line.
.plt.fit:{[x;y] x:"f"$x; y:"f"$y; b:cov[x;y]%var x; a:avg[y]-b*avg x; e:(min x;max x);
  ([] x:e; fit:a+b*e) }

/ One series per group: `.plt.by[`scatter; `sym; select sym,close,vwap from t]` scatters
/ close against vwap once per sym, each its own colour and legend entry. Each series carries
/ its OWN x, which is what makes clusters. A shared x could only stack them on one grid.
.plt.by:{[k;g;x]
  o:()!(); if[.plt.isopt x; o:((key x) except `data)#x; x:x`data];
  t:0!x;
  if[not g in cols t; '"plt.by: no ",(string g)," column"];
  c:(cols t) except g;
  if[2>count c; '"plt.by: need an x and a y column besides ",string g];
  xc:c 0; yc:c 1;
  gt:g xgroup t;
  d:value gt;                                  / one row per group, columns nested per group
  f:.plt.xf first d xc;
  / `key` of a keyed table is a TABLE, so the group labels are that table's g column, not
  / the table itself, which arrives as no names at all and quietly loses the legend.
  .plt.draw ((`kind`x`y`names`xfmt`xlabel`ylabel!(k;
    .plt.xv each d xc; "f"$/:d yc; (key gt)g; f; string xc; string yc)),
    .plt.settings[]),o }
/ A scatter matrix: every column against every other in an NxN grid, the diagonal a
/ histogram of that column, and the least-squares line through each pair.
/    .plt.matrix select AAPL, NVDA, TSLA from rets
/ It DRAWS and also returns the fit, keyed by ordered pair. Regressing AAPL on NVDA is not
/ regressing NVDA on AAPL, so both directions are there and they differ. `r` does not: it is
/ symmetric, which is why it rather than the slope is what each panel is annotated with.
/ `fit_line:0b` suppresses the drawn lines; the returned table is unaffected.
/ Ordinary least squares of y on x, as (slope; intercept; correlation). A column with no
/ variance at all (a constant, or one value after nulls) has no slope rather than an
/ infinite one, which would otherwise reach the renderer as a vertical line across a panel.
.plt.ols:{[y;x] vx:var x;
  $[vx<=0; (0f;avg y;0n); [b:cov[y;x]%vx; (b; avg[y]-b*avg x; cov[y;x]%sqrt vx*var y)]]}
.plt.matrix:{[x]
  o:()!(); if[.plt.isopt x; o:((key x) except `data)#x; x:x`data];
  fl:$[`fit_line in key o; 0<>first o`fit_line; 1b];
  o:(key[o] except `fit_line)#o;
  t:0!x; d:flip t;
  / A returns table has a date column in it and a date is not one of the variables, so
  / non-numeric columns are dropped. That is column SELECTION, not a mistyped option, which
  / is why it does not signal the way an unknown key does.
  c:(cols t) where (abs type each d cols t) in 5 6 7 8 9h;
  if[2>count c; '"plt.matrix: need at least two numeric columns"];
  v:"f"$/:d c;
  m:{[v;y] .plt.ols[y] each v}[v] each v;      / m[i][j] is column i regressed on column j
  s:`kind`y`names`corr!(`matrix; v; c; m[;;2]);
  if[fl; s:s,`beta`alpha!(m[;;0];m[;;1])];
  .plt.draw (s,.plt.settings[]),o;
  / The diagonal is a variable on itself: beta 1, intercept 0, r 1, and nothing to learn.
  row:{[c;m;i] ([] y:count[c]#c i; x:c; beta:m[i][;0]; intercept:m[i][;1]; r:m[i][;2])};
  f:raze row[c;m] each til count c;
  `y`x xkey delete from f where y=x }
/ Lines and translucent bands in one chart, described rather than inferred:
/    .plt.bands ([data:t; style:`price`2sd!(([kind:`line; y:`px]);
/                                           ([kind:`band; y:`px; lo:`lo; hi:`up]))])
/ For ONE series write `([price: ([kind:`band; y:`px; lo:`lo; hi:`up])])`: `` `price!d `` is
/ an atom keyed to a list and signals `type, which is q rather than anything here.
/ The style dict is keyed by SERIES NAME, which is also the legend label. Each value names
/ which columns of `data` play which part:
/    kind  `line | `band      NOT `type: that is a q reserved word and `([type:`x])` signals
/    y     a column           the line; optional for a band, which is then just the region
/    lo,hi columns            a band's edges
/    c     "#4c9aff"          optional; the palette in order otherwise
/ x is the first column of `data`, or whatever `x` names.
/ .
/ q collapses a dict whose values share their keys into a TABLE, so two bands and no line
/ arrive shaped differently from a band and a line. Both index the same way, and `.plt.stl`
/ is where that stops mattering.
/ A column named by the style, cast to float, or `()` when the key is absent.
.plt.col:{[t;e;k] $[(k in key e) and (e k) in cols t; "f"$t e k; ()]}
.plt.bands:{[x]
  o:()!(); if[.plt.isopt x; o:((key x) except `data)#x; x:x`data];
  st:o`style; o:(key[o] except `style)#o;
  if[not 99h=type st; '"plt.bands: needs a `style dictionary"];
  t:0!x; c:cols t;
  nm:key st; ents:value st;
  / A table value indexes row-wise, a dict of dicts entry-wise; `ents i` is the entry either way.
  e:{[ents;i] ents i}[ents] each til count nm;
  xc:$[`x in key o; o`x; first c];
  if[not xc in c; '"plt.bands: no ",(string xc)," column for the x axis"];
  f:.plt.xf t xc;
  ys:{[t;e] $[`y in key e; .plt.col[t;e;`y]; ()]}[t] each e;
  los:.plt.col[t;;`lo] each e;
  his:.plt.col[t;;`hi] each e;
  cs:{$[`c in key x; $[10h=abs type x`c; x`c; string x`c]; ""]} each e;
  if[any (0=count each los)<>0=count each his;
    '"plt.bands: a band needs both `lo and `hi"];
  .plt.draw ((`kind`x`y`lo`hi`names`colours`xfmt`xcats`xlabel!(`bands;
    $[f=`cat; "f"$til count t; .plt.xv t xc];
    ys; los; his; nm; cs; f;
    $[f=`cat; .plt.cats t xc; ()]; string xc)),.plt.settings[]),o }
/ Explicit x/y, when the data isn't already a table: `.plt.xy[`scatter;x;y]`.
.plt.xy:{[k;x;y] .plt.draw `kind`x`y`xfmt!(k; .plt.xv x; "f"$$[0h=type y; y; enlist y]; .plt.xf x)}
