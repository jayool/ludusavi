# Logging

Ludusavi Sync writes log files to the
[application folder](application-folder.md). The current log is named
`ludusavi_rCURRENT.log`; older logs are timestamped, e.g.
`ludusavi_r2000-01-02_03-04-05.log`.

By default, only warnings and errors are logged. Set the `RUST_LOG`
environment variable to change the level — for example,
`RUST_LOG=ludusavi=debug` for verbose output.

The most recent five log files are kept; older ones are rotated out
either when the application starts or when a single log reaches 10 MiB.

The [daemon](daemon.md) writes to the same folder using the same rules.
Daemon log lines are tagged so you can tell them apart from the GUI's
lines when both are running.
