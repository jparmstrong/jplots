/ demo.q: every chart jplots draws, on sample data. Run it from the repo root:
/    JPLOTS_LIB=$PWD/target/release/libjplots q examples/demo.q -q
/    JPLOTS_LIB=$PWD/target/release/libjplots q examples/demo.q -q kitty
/ Once installed (`make install`), `q demo.q -q` is enough, because `2:` finds the library in
/ $QHOME/l64 and `\l plt.q` finds the front end beside it.
/ The renderer defaults to whatever this terminal draws, so neither of these needs an argument.
/ Naming one is for forcing the other, or for capturing a stream to a file. See the README.
/ (No bare "/" lines in this file: a lone slash opens a block comment to EOF.)

/ `q/plt.q` in the source tree, `plt.q` beside this file in a release tarball, and `plt.q` on
/ q's own path once installed. `\l` is a command rather than a function, so choosing needs
/ `system`.
system "l ",$[count key`:q/plt.q; "q/plt.q"; "plt.q"]

/ The renderer, named on the command line. Anything else signals from the first chart with
/ the list of what is built, which is a better answer than a demo that draws nothing.
if[count .z.x; .plt.renderer:`$first .z.x]

/ ---- sample data --------------------------------------------------------------------
/ Built arithmetically rather than with `rand`, so the demo draws the same picture on every
/ run. A demo whose shape changes each time is one nobody can compare against last time.
.demo.u:{[n;seed] (1_ (n){(1103515245*x+12345) mod 2147483648}\seed) % 2147483648f}
/ Approximately standard normal. The mean of eight independent uniforms is bell-shaped by
/ the central limit, and scaling by sqrt(12*8), the reciprocal of that mean's standard
/ deviation, gives unit variance. Enough for a demo, and being arithmetic rather than
/ `rand` it draws the same picture on every host.
.demo.g:{[n;seed] (sqrt 96)*-0.5+avg .demo.u[n;] each seed+1000*1+til 8}
/ Uniform increments would make the returns histogram flat and the scatter clusters square.
.demo.walk:{[n;seed;start;drift] start+sums drift+0.3*.demo.g[n;seed]}

.demo.days:180
.demo.trade:([] date:2026.01.02+til .demo.days;
                close:.demo.walk[.demo.days;7;100f;0.05];
                vwap: .demo.walk[.demo.days;29;100f;0.03])

.demo.vol:([] sym:`NVDA`AAPL`MSFT`TSLA`AMZN`META`GOOG;
              turnover:47.2 38.5 30.6 29.8 22.3 17.4 16.5)

.demo.pnl:([] day:`MON`TUE`WED`THU`FRI; usd:12.4 -5.1 8.8 -11.9 3.2)

/ A benchmark table keyed by query and cache: the keys label each row, the value is the bar,
/ and the LAST key colours it, so cold and warm come out in two colours with a key.
.demo.bench:([query:`vwap`vwap`ohlc`ohlc`asof`asof; cache:`cold`warm`cold`warm`cold`warm]
               seconds:171.55 92.134 240.65 164.31 330.96 57.271)

/ Three names, each with its own price level, so one scatter cluster per name.
.demo.cloud:{[sym;seed;cx;cy] ([] sym:60#sym;
    close:cx+.demo.g[60;seed]; vwap:cy+.demo.g[60;seed+7])}
.demo.quotes:(.demo.cloud[`AAPL;11;100;101]),(.demo.cloud[`NVDA;29;115;113]),
             .demo.cloud[`GOOG;47;92;95]

/ Two measures per category: one column each, and they draw side by side.
.demo.rev:([] sym:`NVDA`AAPL`MSFT`TSLA`AMZN; q1:12.4 9.1 7.8 5.2 6.6; q2:14.9 8.3 8.9 4.1 7.2)

/ Four names driven by one common market move plus their own noise, which is what makes the
/ scatter matrix show anything: with independent columns every panel is a shapeless blob.
.demo.mkt:.demo.g[.demo.days;3]
.demo.co:{[b;seed] b*.demo.mkt+(1f%b)*.demo.g[.demo.days;seed]}
.demo.rets:([] AAPL:.demo.co[0.9;61]; NVDA:.demo.co[1.7;67];
               TSLA:.demo.co[1.4;71]; GOOG:.demo.co[0.8;73])

/ Daily returns in percent. The histogram takes the raw observations, not counts.
/ The `1_` matters: `ratios` yields x[0] itself as its first element, not a ratio, so day one
/ would arrive as 100*(100-1) = 9880: one outlier wide enough to collapse every real bin
/ into the leftmost bar.
.demo.ret:100f*-1+1_ratios exec close from .demo.trade

/ ---- the charts ---------------------------------------------------------------------
.plt.size:900 420

-1 "1/12  line: x is the first column, one series per remaining column";
.plt.line select date, close, vwap from .demo.trade

-1 "2/12  the same, styled: a dict of `data` plus per-plot options";
.plt.line ([
  title:  "ACME - close vs vwap";  / the bitmap font is ASCII; a dash, not an em-dash
  ylabel: "usd";
  data:   select date, close, vwap from .demo.trade ])

-1 "3/12  bar: a keyed table plots directly, its symbol key becoming a categorical axis";
/ Note the `sum`: `select turnover by sym` without an aggregate gives one NESTED LIST per
/ group, which is a list of series rather than a series. That is q, not jplots.
.plt.bar ([
  title:  "turnover by name";
  ylabel: "usd (bn)";
  data:   select sum turnover by sym from `turnover xdesc .demo.vol ])

-1 "4/12  bar: signed values sit either side of a zero line";
.plt.bar ([title:"P&L by day"; ylabel:"usd (m)"; data:.demo.pnl])

-1 "5/12  bar: several value columns become side-by-side groups with a key";
.plt.bar ([title:"revenue by quarter"; ylabel:"usd (bn)"; data:.demo.rev])

-1 "6/12  hbar: one bar per row, top row first, labelled by its keys and coloured by the last";
.plt.hbar ([title:"query runtime"; data:.demo.bench])

-1 "7/12  histogram: pass the observations; the bins are computed for you";
.plt.hist ([title:"daily returns"; xlabel:"pct"; bins:40; data:.demo.ret])

-1 "8/12  scatter: two columns of a table are x and y";
.plt.scatter ([title:"close vs vwap"; data:select close, vwap from .demo.trade])

-1 "9/12  scatter with a fitted line over it: `overlay` draws its series as lines on top";
.plt.scatter ([title:"close vs vwap, least squares"; data:select close, vwap from .demo.trade;
  overlay:.plt.fit[.demo.trade`close; .demo.trade`vwap]])

-1 "10/12  scatter matrix: every pair of return series, with its least-squares line";
/ It draws AND returns the fit per ordered pair: the regression you would otherwise write out
/ by hand to read a number off the picture. `r` is symmetric; the slopes are not.
.plt.size:820 820
.demo.fit:.plt.matrix ([title:"daily % returns"; data:.demo.rets])
.plt.size:900 420
-1 .Q.s 2#.demo.fit;
-1 "";

-1 "11/12  scatter by group: one series per sym, each cluster its own colour and key entry";
.plt.by[`scatter; `sym; ([title:"close vs vwap by name"; data:.demo.quotes])]

-1 "12/12  bands: a translucent region per series, from a `style dictionary";
/ A rolling mean with 2 standard deviation edges. `.plt.bands` reads WHICH columns are the
/ centre and the edges from `style` rather than taking them positionally, so a band and a
/ plain line can sit in one chart.
.demo.band:update mid:20 mavg close, sd:20 mdev close from select date, close from .demo.trade
.demo.band:update up:mid+2*sd, lo:mid-2*sd from .demo.band
.plt.bands ([
  title:  "close against its rolling 2 sd band";
  ylabel: "usd";
  data:   20_ select date, close, mid, lo, up from .demo.band;   / drop the partial window
  style:  ([close: ([kind:`band; y:`close; lo:`lo; hi:`up])]) ])

-1 "";
-1 "settings are ordinary globals: .plt.theme, .plt.size, .plt.font, .plt.scale, .plt.bins";
-1 "`.plt.info[]` reports what this terminal was detected to be:";
-1 .Q.s .plt.info[];
