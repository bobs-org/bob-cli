---
origin: requested
scope: project
aliases:
  - ack
  - unacked
  - event acknowledgement
  - gcal ack
---
# Event Ack

Bob's one-time review answer for an external Google Calendar event that will
land as an [[event-task]]. A live `gcal.ack` receipt or any persisted
nonempty private event note counts, including note text later cleared; a
series note covers its future instances. Until answered, an open Event Task
carries Bob's `#unacked` warning tag. Removing that exact tag is itself an
acknowledgement. The review is a reminder, not an RSVP or a sync gate: Bob
still writes eligible Event Tasks, and never writes to Google. See
`docs/gcal-acks.md`.
