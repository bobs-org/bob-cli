# Podcast fixtures

- `pragmatic_engineer_notes.html`: the `htmlDescription` of Spotify episode
  `4UgVFWty2I8hGAgSvT7pjC` (The Pragmatic Engineer, 2026-11-18), captured
  2026-11-21 and kept byte-for-byte, including its 16 U+2060 characters. It
  holds a "Brought to You By:" sponsor run ending at a `—` line, and a
  "Timestamps" label over 16 chapter lines from 0:00 to 1:24:30.
  `podcast/notes.rs` tests segment it.
- `itunes_pragmatic_engineer.json`: the iTunes Search response for
  `term=The Pragmatic Engineer` (`media=podcast&entity=podcast&limit=10`),
  captured 2026-11-21 and trimmed to each result's kind, ids, `artistName`,
  `collectionName`, and `feedUrl`.
- `feed_pragmatic_engineer.rss`: the show's public RSS feed
  (`api.substack.com/feed/podcast/458709.rss`), captured 2026-11-21 and trimmed
  to its namespaces, a channel title, and the newest three items. Each item
  keeps its `title`, `description`, `guid`, `pubDate`, `podcast:transcript`,
  and `itunes:duration`; the first item's `description` is unchanged (it is the
  episode's show notes, with U+2060 characters), and the later two keep one
  paragraph. `podcast/feed.rs` tests match it against the Spotify episode.
- `chapters.json`: a synthetic Podcasting 2.0 chapters file with a fractional
  start, a `"toc": false` chapter, an untitled chapter, and a U+2060 in a
  title.
- `pragmatic_engineer_transcript.vtt`: the first 60 cues (0:00–3:00) of the
  `podcast:transcript` VTT that the first item of `feed_pragmatic_engineer.rss`
  offers, captured 2026-11-21 and kept byte-for-byte. It is diarized:
  `<v SPEAKER_01>` (the host, speaking first) and `<v SPEAKER_00>`. The full
  file has 1,750 cues; its last starts at 1:26:35. `podcast/transcript.rs`
  tests parse it.
- `transcript.srt`, `transcript.json`, `transcript.html`, `transcript.txt`:
  synthetic re-encodings of that VTT's text, one per other transcript
  format. The SRT holds the first 45 cues with CRLF line endings and no
  speakers. The JSON holds every cue as a Podcasting 2.0 segment with
  diarized `spk_0`/`spk_1` speakers. The HTML holds one `<cite>`/`<time>`/
  `<p>` group per speaker turn, with the speakers named. The text holds
  one blank-line paragraph per speaker turn, with no times or speakers.
