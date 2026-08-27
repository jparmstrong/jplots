/ candlestick.q: OHLC candlesticks from 5-minute bars. Run it from the repo root:
/    JPLOTS_LIB=$PWD/target/release/libjplots q examples/candlestick.q -q
/ (No bare "/" lines in this file: a lone slash opens a block comment to EOF.)

/ `q/plt.q` in the source tree, `plt.q` beside this file in a release tarball, and `plt.q` on
/ q's own path once installed. `\l` is a command rather than a function, so choosing needs
/ `system`.
system "l ",$[count key`:q/plt.q; "q/plt.q"; "plt.q"]

/ The renderer, named on the command line, as in demo.q:  q examples/candlestick.q -q kitty
if[count .z.x; .plt.renderer:`$first .z.x]

/ ---- a day of 5-minute bars ----------------------------------------------------------
/ Built arithmetically so the picture is the same on every run. A real feed would arrive as
/ trades, and the aggregation below is how you would get from those to bars.
.ex.u:{[n;seed] (1_ (n){(1103515245*x+12345) mod 2147483648}\seed) % 2147483648f}

/ 09:30 to 16:00 is 78 five-minute buckets.
.ex.n:78
.ex.time:09:30:00.000+300000*til .ex.n

/ A mid price that drifts, then an open and close either side of it, then a high and low
/ that must bracket both, which is the invariant a candle depends on.
.ex.mid:  100f+sums 0.02+.ex.u[.ex.n;11]-0.5
.ex.open: .ex.mid
.ex.close:.ex.mid+0.6*.ex.u[.ex.n;23]-0.5
.ex.bars:([] time: .ex.time;
             open: .ex.open;
             high: (.ex.open|.ex.close)+0.45*.ex.u[.ex.n;37];
             low:  (.ex.open&.ex.close)-0.45*.ex.u[.ex.n;41];
             close:.ex.close;
             volume:1000+`long$8000*.ex.u[.ex.n;53])

/ From raw trades you would build the same table with a `by` on the bucket:
/    select open:first px, high:max px, low:min px, close:last px, volume:sum sz
/      by 5 xbar time.minute from trade where sym=`ACME
/ `.plt.candle` picks its four columns out by NAME, so the order they appear in does not
/ matter and the leftover column (`time` here) becomes the x axis.

-1 "1/3  candlesticks: hollow is an up bar, filled is a down bar";
.plt.size:900 400
.plt.candle ([title:"ACME - 5-minute bars"; ylabel:"usd"; data:select time,open,high,low,close from .ex.bars])

-1 "2/3  the same day at 15 minutes, so the bodies have room to read";
.ex.q15:select open:first open, high:max high, low:min low, close:last close
        by 15 xbar time.minute from .ex.bars
.plt.candle ([title:"ACME - 15-minute bars"; ylabel:"usd"; data:.ex.q15])

-1 "3/3  volume underneath, as an ordinary bar chart on the same buckets";
.plt.size:900 200
.plt.bar ([title:"volume"; ylabel:"shares"; data:select time, volume from .ex.bars])
