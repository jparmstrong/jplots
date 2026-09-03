/ test.q: the q-visible contract.
/   q tests/test.q -q     (with JPLOTS_LIB set, or libjplots on the path)
/ Everything that draws is pinned to a tiny frame: a test run should not spray image data.
\l q/plt.q
.t.n:0; .t.bad:();
.t.ok:{[n;c] .t.n+:1; if[not c; .t.bad,:enlist n]; }
.t.eq:{[n;a;b] .t.ok[n; a~b]; }

t:([] date:2026.01.01+til 5; close:1 2 3 4 5f; vwap:2 3 4 5 6f)

.t.section:"spec normalisation"
s:.plt.mk[`line;t]
.t.eq["table: x is the first column";  s`x;      "f"$2026.01.01+til 5]
.t.eq["table: y is the rest";          s`y;      (1 2 3 4 5f;2 3 4 5 6f)]
.t.eq["table: names";                  s`names;  `close`vwap]
.t.eq["table: date axis";              s`xfmt;   `date]
.t.eq["one column: no x";              .plt.mk[`line; select close from t]`x;  ()]
.t.eq["vector";                        .plt.mk[`line; 1 2 3f]`y;               enlist 1 2 3f]
.t.eq["list of vectors";               .plt.mk[`line; (1 2 3f;4 5 6f)]`y;      (1 2 3f;4 5 6f)]
.t.eq["dict keys are x";               .plt.mk[`line; (1 2 3f)!4 5 6f]`x;      1 2 3f]
/ A keyed table unkeys first, so `select … by date` plots directly. The BAR is positional and
/ so carries labels; the calendar axis is what a line makes of the same table.
.t.eq["keyed table unkeys";            .plt.mk[`bar; select sum close by date from t]`xcats;
                                       string 2026.01.01+til 5]
.t.eq["and a line keeps the calendar"; .plt.mk[`line; select sum close by date from t]`xfmt; `date]
.t.eq["symbol key is categorical";     .plt.mk[`bar; `a`b`c!1 2 3f]`xcats;     string `a`b`c]
/ `string` of a list of strings splits each into characters, and the labels were lost.
.t.eq["string labels survive";         .plt.mk[`bar; ([] k:("ab";"cd"); v:1 2f)]`xcats; ("ab";"cd")]
/ A single-series chart draws no legend, so without this its column name is nowhere.
.t.eq["one series names the y axis";   .plt.mk[`line; select close from t]`ylabel;  "close"]
.t.eq["scatter: x and y both named";   `xlabel`ylabel#.plt.mk[`scatter; select close, vwap from t];
                                       `xlabel`ylabel!("close";"vwap")]
.t.ok["several series: no ylabel";     not `ylabel in key .plt.mk[`line;t]]
.t.eq["an explicit ylabel wins";       .plt.mk[`line; ([data:select close from t; ylabel:"usd"])]`ylabel; "usd"]

.t.section:"axis kind from the column type"
.t.eq["date";       .plt.xf 2#2026.01.01;        `date]
.t.eq["time";       .plt.xf 2#12:00:00.000;      `time]
.t.eq["timestamp";  .plt.xf 2#2026.01.01D0;      `timestamp]
.t.eq["symbol";     .plt.xf `a`b;                `cat]
.t.eq["string";     .plt.xf ("ab";"cd");         `cat]
.t.eq["numeric";    .plt.xf 1 2 3;               `num]
/ `xbar time.minute` is how q buckets, and it yields a MINUTE: a clock axis, not a number.
.t.eq["minute";     .plt.xf 2#12:00;             `time]
.t.eq["second";     .plt.xf 2#12:00:00;          `time]
.t.eq["timespan";   .plt.xf 2#0D12:00:00.0;      `time]
/ …scaled to the milliseconds the renderer's clock axis expects.
.t.eq["minute -> ms";   .plt.xv 2#12:00;         2#43200000f]
.t.eq["second -> ms";   .plt.xv 2#12:00:00;      2#43200000f]
.t.eq["time is already ms"; .plt.xv 2#12:00:00.000; 2#43200000f]

.t.section:"one series per group"
/ `.plt.by` gives each group its OWN x. A shared x could only stack the clusters on one
/ grid, which is the whole point of plotting them together.
.pl.g:([] sym:`a`a`b`b; close:1 2 3 4f; vwap:5 6 7 8f)
.t.eq["by: draws";                .plt.by[`scatter; `sym; .pl.g];  (::)]
.t.ok["by: needs its group column"; (@[.plt.by[`scatter;`nosuch;]; .pl.g; {x}]) like "*nosuch*"]
.t.ok["by: needs an x and a y";     (@[.plt.by[`scatter;`sym;]; select sym,close from .pl.g; {x}]) like "*besides*"]

.t.section:"the options dict"
.t.eq["title rides along";   .plt.mk[`line; ([title:"T"; data:t])]`title;  "T"]
.t.eq["order is irrelevant"; .plt.mk[`line; ([data:t; title:"T"])]`title;  "T"]
.t.eq["a table is not options";       .plt.isopt t;                0b]
.t.eq["a data dict is not options";   .plt.isopt `a`b!1 2f;        0b]
.t.eq["`data makes it options";       .plt.isopt ([data:1 2 3f]);  1b]
.t.ok["an unknown option signals"; (@[.plt.mk[`line;]; ([data:1 2 3f; titel:"x"]); {x}]) like "*unknown option*"]

.t.section:"settings reach the spec"
.plt.size:64 48
.t.eq["size merged";      .plt.mk[`line;t]`size;   64 48]
.t.eq["theme merged";     .plt.mk[`line;t]`theme;  `dark]
.t.eq["per-call wins";    .plt.mk[`line; ([data:t; theme:`light])]`theme;  `light]
.t.eq["session untouched"; .plt.theme;             `dark]

.t.section:"drawing"
.t.eq["draw returns ::";         .plt.draw .plt.mk[`line;t];  (::)]
.t.eq["line";                    .plt.line t;                 (::)]
.t.eq["scatter";                 .plt.scatter t;              (::)]
.t.eq["bar";                     .plt.bar `a`b!1 2f;          (::)]
.t.eq["hist";                    .plt.hist 1 2 3 4 5 6 7 8f;  (::)]
.t.eq["hbar";                    .plt.hbar `a`b!1 2f;         (::)]
.t.eq["empty series";            .plt.line 0#0f;              (::)]
.t.eq["single point";            .plt.line enlist 1f;         (::)]
.t.eq["all nulls";               .plt.line 3#0n;              (::)]
.t.eq["one-column table";        .plt.line select close from t; (::)]
.t.eq["string column";           .plt.line ([] name:("ab";"cd"); v:1 2f); (::)]
.t.eq["mismatched x and y";      .plt.xy[`line; enlist 1f; 0#0f]; (::)]
.t.ok["a bad kind signals";      (@[.plt.draw; `kind`y!(`pie; enlist 1 2f); {x}]) like "*kind*"]
.t.ok["a spec with no y signals"; (@[.plt.draw; (enlist `kind)!enlist `line; {x}]) like "*y*"]

.t.section:"overlay"
f:.plt.fit[1 2 3 4 5f; 2 4 6 8 10f]
.t.eq["fit is a 2-row table";  count f;                   2]
.t.eq["fit passes through";    "j"$f`fit;                 2 10]
o:.plt.mk[`scatter; ([data:t; overlay:f])]
.t.eq["overlay flags trail";   o`overlay;                 001b]
.t.eq["overlay adds a series"; count o`y;                 3]
.t.eq["overlay is named";      last o`names;              `fit]
.t.eq["x is now per-series";   count o`x;                 3]
.t.eq["overlay is not an opt"; `overlay in key o;         1b]
.t.eq["draws over a scatter";  .plt.scatter ([data:t; overlay:f]);  (::)]
.t.eq["draws over a bar";      .plt.bar ([data:`a`b!1 2f; overlay:([] i:0 1f; m:1 2f)]); (::)]
.t.eq["overlay on a vector";   .plt.line ([data:1 2 3f; overlay:2 2 2f]); (::)]
.t.ok["a bad option still signals"; (@[.plt.line; ([data:t; titel:"x"]); {x}]) like "*unknown option*"]

.t.section:"bands"
bd:([] d:2026.01.01+til 5; px:1 2 3 4 5f; lo:0 1 2 3 4f; up:2 3 4 5 6f)
/ One series is `([name: entry])`: `` `name!entry `` is an atom keyed to a list and signals.
one:([price: ([kind:`band; y:`px; lo:`lo; hi:`up])])
.t.eq["bands draws";          .plt.bands ([data:bd; style:one]);  (::)]
/ `.plt.bands` builds and draws in one step, so the spec is checked by drawing without error
/ and by the shapes it accepts. A band needs BOTH edges.
.t.ok["lo without hi signals";
/ One interior wildcard only: kdb's `like` signals `nyi on a pattern with two, so `*a*b*`
/ is not a thing that can be asserted even though it reads like one.
  (@[.plt.bands; ([data:bd; style:([p: ([kind:`band; y:`px; lo:`lo])])]); {x}]) like "*needs both*"]
/ y is OPTIONAL: a band with no centre line is just the region.
.t.eq["a band needs no centre";
  .plt.bands ([data:bd; style:([p: ([kind:`band; lo:`lo; hi:`up])])]);  (::)]
/ Lines and bands mix, and an explicit colour rides along.
.t.eq["a line beside a band";
  .plt.bands ([data:bd;
    style:`price`sd!(([kind:`line; y:`px; c:"#e5484d"]); ([kind:`band; lo:`lo; hi:`up]))]); (::)]
/ x is the first column, or whatever `x` names.
.t.eq["x can be named";
  .plt.bands ([data:([] px:1 2 3f; when:2026.01.01+til 3; lo:0 1 2f; up:2 3 4f);
    x:`when; style:([p: ([kind:`band; lo:`lo; hi:`up])])]); (::)]
.t.ok["a missing x column signals";
  (@[.plt.bands; ([data:bd; x:`nope; style:one]); {x}]) like "*nope*"]
.t.ok["style must be a dict";
  (@[.plt.bands; ([data:bd; style:1 2 3]); {x}]) like "*style*"]

.t.section:"bar axes"
/ A bar sits at a POSITION, so a bar chart is categorical whatever it was made from. Without
/ that the renderer formats the INDEX in the column's own type, and daily volume for 2026 came
/ out with an axis reading 2000.01.01.
bt:([] d:2026.08.01 2026.08.02 2026.08.03; v:1 2 3f)
.t.eq["a bar's x is categorical";  (.plt.mk[`bar;bt])`xfmt;   `cat]
.t.eq["and carries real labels";   (.plt.mk[`bar;bt])`xcats;  ("2026.08.01";"2026.08.02";"2026.08.03")]
.t.eq["bars sit at 0 1 2";         (.plt.mk[`bar;bt])`x;      0 1 2f]
/ Every other kind keeps the calendar axis: only bars are positional.
.t.eq["a line keeps its dates";    (.plt.mk[`line;bt])`xfmt;  `date]
.t.eq["a scatter keeps its dates"; (.plt.mk[`scatter;bt])`xfmt; `date]
/ The same for a dict, which is what `exec v by k` gives.
.t.eq["a dict bar is categorical"; (.plt.mk[`bar; 2026.08.01 2026.08.02!1 2f])`xfmt; `cat]

.t.section:"hbar"
/ Every non-numeric column is part of the row's label, joined with a space, and the numeric
/ ones are the bars. The categories ride as `xcats` and the renderer puts them on y, so the
/ axis NAMES swap: the value column names x, the label columns name y.
ht:([] query:`q1`q1`q2; runtime:("CQ";"REF";"CQ"); elapsed:1 2 3f)
.t.eq["labels join the text columns"; (.plt.hl ht)`$"query runtime"; ("q1 CQ";"q1 REF";"q2 CQ")]
.t.eq["numeric columns are the bars";  cols .plt.hl ht; `$("query runtime";"elapsed")]
.t.eq["no label column: row index";    (.plt.hl ([] v:1 2f))`row; (enlist "0";enlist "1")]
.t.ok["no numeric column signals";     (@[.plt.hl; ([] a:`x`y); {x}]) like "*numeric*"]
.t.eq["hbar draws a table";            .plt.hbar ht; (::)]
.t.eq["hbar with options";             .plt.hbar ([title:"t"; data:ht]); (::)]
.t.eq["hbar draws a keyed table";      .plt.hbar select sum elapsed by query from ht; (::)]
/ A keyed table colours its bars by the LAST key, so `by query, cache` is coloured by cache.
kt:([q:`a`a`b`b; cache:`cold`warm`cold`warm] v:1 2 3 4f)
.t.eq["keyed: groups are the last key"; {[t] .plt.draw:{x`groups}; r:.plt.hbar t; r}[kt]; ("cold";"warm";"cold";"warm")]
.plt.draw:.plt.lib 2:(`draw;1)
.t.ok["unkeyed: no groups";            not `groups in key {.plt.draw:{x}; r:.plt.hbar x; r}[ht]]
.plt.draw:.plt.lib 2:(`draw;1)
.t.eq["keyed hbar draws";              .plt.hbar kt; (::)]
.t.eq["hbar draws a vector";           .plt.hbar 1 2 3f; (::)]

.t.section:"backends"
/ `.plt.renderer` picks the escape sequence, not the drawing: both backends read the same raster,
/ so a chart that draws under one draws under the other.
/ The default is DETECTED, so it cannot be pinned to a value: this suite has to pass in a
/ kitty terminal too. The rule is a function precisely so it can be asked without an
/ environment to set up.
.t.eq["kitty terminals detected"; .plt.rend "xterm-kitty|";            `kitty]
.t.eq["ghostty detected";         .plt.rend "xterm-256color|ghostty";  `kitty]
.t.eq["everything else is sixel"; .plt.rend "xterm-256color|";         `sixel]
.t.ok["the default is one of them"; .plt.renderer in `sixel`kitty]
.t.eq["kitty draws"; {r0:.plt.renderer; .plt.renderer:`kitty; r:.plt.line t; .plt.renderer:r0; r}[]; (::)]
/ Named `renderer` rather than `type`: `type` is a q reserved word, so `([… type:`sixel])`
/ is an ASSIGNMENT to it and signals `assign before jplots ever sees it.
.t.eq["per-call in a literal"; .plt.line ([data:t; renderer:`sixel]);  (::)]
.t.eq["per-call in a dict";    .plt.line `data`renderer!(t;`sixel);    (::)]
.t.eq["the toggle restored it"; .plt.renderer;               .plt.rend .plt.trm]
/ Where a sixel image leaves the cursor is not in the protocol and terminals disagree by a
/ whole image height, so it is a setting. `jplots-probe --cursor` is how to find yours.
.t.eq["cursor default";       .plt.sixel_cursor;                      `reserve]
.t.eq["cursor per call";      .plt.line ([data:t; renderer:`sixel; sixel_cursor:`advance]); (::)]
.t.ok["an unknown cursor signals";
  (@[.plt.line; ([data:t; renderer:`sixel; sixel_cursor:`nope]); {x}]) like "*unknown*"]
.t.ok["svg says so";        (@[.plt.line; ([data:t; renderer:`svg]); {x}]) like "*not built yet*"]
/ `text` was once listed as planned and is not coming, so it is an unknown name now, not a
/ promise that has yet to be kept.
.t.ok["text is not a plan";  (@[.plt.line; ([data:t; renderer:`text]); {x}]) like "*unknown renderer*"]
.t.ok["an unknown renderer signals"; (@[.plt.line; ([data:t; renderer:`png]); {x}]) like "*unknown renderer*"]

.t.section:"matrix"
/ Deterministic: `?` is random, and a flaky pair would fail the "beta is not symmetric"
/ check whenever the two draws happened to match.
ma:"f"$40#3 1 4 1 5 9 2 6 5 3
mt:([] date:2026.01.01+til 40; a:ma; b:ma+"f"$40#2 7 1 8 2 8; c:"f"$40#1 6 1 8 0 3 3 9)
mf:.plt.matrix ([data:mt; title:"m"])
.t.eq["one row per ordered pair"; count mf;            6]
.t.eq["the diagonal is dropped";  count select from 0!mf where x=y;  0]
.t.eq["keyed on the pair";        keys mf;             `y`x]
.t.eq["fit columns";              cols value mf;       `beta`intercept`r]
/ r is symmetric and the slope is not: regressing a on b is not regressing b on a, which is
/ why the panels are annotated with r rather than with the slope.
.t.ok["r is symmetric";  0.000001>abs (mf[`a`b]`r) - mf[`b`a]`r]
.t.ok["beta is not";     0.01<abs (mf[`a`b]`beta) - mf[`b`a]`beta]
/ Least squares of a perfect line recovers it exactly.
lt:([] p:"f"$til 20; q:3+2*"f"$til 20)
lf:.plt.matrix lt
.t.ok["exact slope";     0.000001>abs 2-(lf[`q`p]`beta)]
.t.ok["exact intercept"; 0.000001>abs 3-(lf[`q`p]`intercept)]
.t.ok["exact r";         0.000001>abs 1-(lf[`q`p]`r)]
.t.eq["fit_line off still draws"; count .plt.matrix ([data:lt; fit_line:0b]); 2]
/ A date column is not one of the variables; a constant column has no slope rather than an
/ infinite one, which would otherwise reach the renderer as a line across the panel.
.t.eq["dates are not variables"; count .plt.matrix ([] d:2026.01.01+til 10; a:"f"$til 10; b:10?1f);  2]
.t.eq["a constant column";       0f;  .plt.matrix[([] k:10#1f; v:"f"$til 10)][`v`k]`beta]
.t.ok["too few columns signals"; (@[.plt.matrix; ([] d:2026.01.01+til 5; a:"f"$til 5); {x}]) like "*two numeric*"]

.plt.size:()
-1 "";
-1 $[count .t.bad;
     "FAILED ",(string count .t.bad)," of ",(string .t.n),": ","; " sv .t.bad;
     "ALL ",(string .t.n)," PASSED"];
exit count .t.bad
