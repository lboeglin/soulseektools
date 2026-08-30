# Why the Nicotine+ plugin was replaced

The first version of this project drove downloads from a Nicotine+ plugin. It
worked, but every item below was hit during real use, and together they are the
reason the download engine moved to [slskd](slskd-setup.md), which runs headless
and exposes an HTTP API that can be tested without a GUI process.

Kept because the behaviour is real and worth knowing about if you ever go back
to Nicotine+, or hit something similar elsewhere.

## Nicotine+ persists its download queue across restarts

The queue lives in `downloads.json`. Killing the process does *not* cancel an
in-flight or queued transfer: on the next launch it resumes everything not
marked "Finished", including stale duplicate requests from an earlier candidate
choice. This caused repeated duplicate downloads.

If you restart after switching candidates, check `downloads.json` for old
non-Finished entries pointing at the same destination file and remove them, or
you get both.

*How this project avoids it:* approvals live in the review queue, one candidate
per track, and re-approving replaces the previous choice rather than adding to
it. Queue state is read back from the backend rather than inferred.

## Log output lags behind actual activity

Output written through `nohup ... > file.log` lagged from seconds to a couple of
minutes behind reality, because of stdout buffering. Something that looked
stalled in the log had often already finished.

*How this project avoids it:* progress is written to the terminal as it happens,
and `--json` output is produced only once a command has finished.

## Files get transiently locked

"Permission denied" while Nicotine+ is uploading a file to another Soulseek user
is expected when your library is shared. Retry later; do not force anything.

*Unchanged:* still true of any client that shares your library. Commands that
write to files report a per-file failure and carry on rather than aborting.

## Search exclusion syntax is unreliable

Soulseek's `-word` exclusion consistently returned zero matches rather than
filtering, when used to exclude "remix", "live" and similar from results.

*How this project avoids it:* searches are plain, and unwanted results are
filtered locally by `ranking.reject_terms` and `ranking.penalise_terms`, matched
against the full remote path. Filtering after the fact is also strictly more
informative — a rejected candidate is still shown, with the reason.

## No Soulseek privileges means unpredictable queues

Without privileges, downloads queue behind privileged users on each peer. Wait
time per file is unpredictable and cannot be forced. Switching to another
uploader is usually faster than waiting out a stalled one.

*Unchanged, but visible:* each candidate shows the peer's queue length and
whether it has a free upload slot, and both feed into the ranking, so the
fastest source tends to be offered first. Switching is a keystroke.

## The ranker only scored the filename

Named in the original README as the root cause of most wrong matches. A file
named exactly right can still be a live concert recording, a different artist's
remix, or a medley track, and none of that is visible from the filename. Folder
names usually say so outright: "Live", "Symphony", "Orchestra", a different
circle or label.

*Fixed:* `soulseektools/soulseek/ranking.py` scores the whole remote path, folder
included, and the review app shows that path against every candidate.
