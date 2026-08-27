/ report.q: a data quality check that mails itself.
/    q examples/report.q                     look at it in the terminal
/    q examples/report.q | jplots-mail | sendmail -t
/ The SAME file is the thing you develop interactively and the thing cron runs. A renderer can
/ still be named on the command line to force one, but neither that nor `-q` is needed: the
/ default is detected, and `jplots-mail` decodes whichever one it is back to pixels.
/ (No bare "/" lines in this file: a lone slash opens a block comment to EOF.)

\l q/plt.q
if[count .z.x; .plt.renderer:`$first .z.x]

/ Light theme and a pinned size: mail is read on white, and with stdout redirected there is no
/ terminal to measure, so the auto-fit size would be a guess.
.plt.theme:`light
.plt.size:640 260

/ ---- the data ------------------------------------------------------------------------
/ Arithmetic rather than `rand`, so the report is the same on every run and a change in it
/ means a change in the code.
.rep.u:{[n;seed] (1_ (n){(1103515245*x+12345) mod 2147483648}\seed) % 2147483648f}
.rep.n:120
t:([] date:2026.05.01+til .rep.n;
      px:  100f+sums 2*-0.5+.rep.u[.rep.n;7];
      qty: `long$1e4*0.3+.rep.u[.rep.n;11])
/ Two things worth catching, both inside the reported window so the charts show what the text
/ says. An anomaly outside the window only widens the band, which is the opposite of a demo.
t:update px:px+4 from t where date within 2026.08.19 2026.08.21
t:update qty:0 from t where date=2026.08.12

/ ---- the check -----------------------------------------------------------------------
/ A rolling 30-day mean with 2 standard deviation bands. A price outside its own recent band
/ is the definition of "worth a look" that needs no threshold picked by hand, and `mdev` is
/ the moving standard deviation, so the band widens where the series is genuinely volatile
/ instead of flagging every busy week.
.rep.w:30
t:update mid:.rep.w mavg px, sd:.rep.w mdev px from t
t:update up:mid+2*sd, lo:mid-2*sd from t
/ The first w-1 rows have an incomplete window, so their band is not a band yet.
t:update band:.rep.w<=1+til count t from t

recent:select from t where date>=max[date]-30
breaches:select date, px, lo, up from recent where band, (px>up)|px<lo
zero:select date from recent where qty=0
errs:count[breaches]+count zero

/ ---- the email -----------------------------------------------------------------------
/ These lines become message headers and are removed from the body, so the script decides its
/ own subject and recipients from its own results.
-1 "EMAIL-TO: ops@example.com";
-1 "EMAIL-FROM: cq@example.com";
-1 "EMAIL-SUBJECT: ",$[errs; "[BAD] ",string[errs]," check failures"; "[OK] daily quality"];

-1 $[errs; "FAILED: ",string[errs]," rows need attention."; "All checks passed."];
-1 "";
-1 "window: last 30 days to ",string max t`date;

/ One section per check, each its own chart followed by the rows that chart is evidence for.
/ `jplots-mail` keeps the order it reads, so a table printed here lands under its own plot
/ rather than in a block of tables above a block of pictures: the reader sees the shape of
/ the problem and the rows to act on together, without scrolling between them.

-1 "";
-1 "1. price against its rolling 2 sd band";
/ `px` is the band's CENTRE line, so a breach reads as the price leaving its own region
/ rather than as three lines crossing. Going out of bounds is the point of the check.
.plt.bands ([
  title:  "px against its rolling 2 sd band";
  ylabel: "usd";
  data:   select date, px, lo, up from recent where band;
  style:  ([price: ([kind:`band; y:`px; lo:`lo; hi:`up])]) ])
-1 $[count breaches; "outside the band:"; "no breaches."];
if[count breaches; -1 .Q.s breaches];

-1 "";
-1 "2. daily volume";
/ `sum` matters: `select qty by date` with no aggregate gives one NESTED LIST per group,
/ which is a list of series rather than a series, and the chart comes out empty. That is q.
.plt.bar ([title:"daily volume"; ylabel:"shares"; data:select sum qty by date from recent])
-1 $[count zero; "zero volume:"; "no zero-volume days."];
if[count zero; -1 .Q.s zero];

-1 "";
-1 "rows checked: ",string count recent;
exit 0
