# Spotify fixtures

Trimmed public responses for episode `4UgVFWty2I8hGAgSvT7pjC` (_Software
factories and code reviews with Harjot Gill_, The Pragmatic Engineer), captured
2026-11-21 with bob's plain `curl` user agent, plus synthetic drift cases.
`podcast/metadata.rs` tests read them with `include_str!`.

- `episode_page.html`: the episode page, trimmed to the meta tags, the JSON-LD
  `PodcastEpisode`, and an `initialState` re-encoded with only the episode
  entity (field values unchanged, including the 16 U+2060 characters in its
  `htmlDescription`).
- `episode_embed.html`: `/embed/episode/<id>` with `__NEXT_DATA__` trimmed to the
  entity (no session token or audio URLs).
- `episode_oembed.json`: the oEmbed response, unchanged.
- `drift_page_no_initial_state.html`: the page without `initialState`.
- `drift_page_other_entity.html`: every page source names another episode id.
- `drift_page_json_ld_array.html`: JSON-LD as an array, nothing else.
- `drift_page_shell.html`: the empty app shell a browser user agent gets.
