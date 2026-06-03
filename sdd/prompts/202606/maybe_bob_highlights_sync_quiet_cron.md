---
plan: sdd/tales/202606/maybe_bob_highlights_sync_quiet_cron.md
---
 We added the `maybe_bob_highlights_sync` script to my chezmoi repo. This script is run every 15m on my macbook, which I have configured via my user's crontab. That means that it is important that this script not produce any output unless there is a problem (otherwise, I get an email every time). Can you help me make this change? Think this through thoroughly and create a plan using your `/sase_plan` skill. Submit your plan with the `sase plan`
command (as the skill instructs) before making any file changes.
