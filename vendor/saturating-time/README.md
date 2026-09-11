# Vendored `saturating-time` 0.4.0

This is the MIT-licensed API and implementation from `saturating-time` 0.4.0,
with one portability correction in `find_limit`: a successful operation that
does not change the time value is treated like an out-of-range operation.

Windows represents `SystemTime` as 100 ns `FILETIME` ticks. The upstream 0.4.0
loop can therefore keep adding a 1 ns step successfully without making
progress. Arti reaches that loop while validating its first Tor consensus,
which pins a CPU core and prevents bootstrap from advancing past 15%.

The included coarse-clock regression test covers the failure independently of
the host operating system. Remove the Cargo patch when this correction is
available in a released upstream version used by Arti.
