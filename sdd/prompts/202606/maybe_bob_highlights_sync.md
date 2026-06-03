---
plan: sdd/tales/202606/maybe_bob_highlights_sync.md
---
 Can you help me write a new `maybe_bob_highlights_sync` script (add it to my chezmoi repo)?

- My intent is to run this script every hour (via a crontab entry) to sync my ~/bob/ref/ note files with PDF annotations / notes added by the Highlights app.
- This script should try to be lazy by doing something like running the `find ~/bob/lib -type f -mtime 0 | wc -l` command, ensuring that the number is non-zero and terminating otherwise (since no PDF files have been edited in >=24h).
- If we confirm that at least one PDF has been modified in the last 24 hours, then we should run the `bob highlights-ref sync` command.

Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
