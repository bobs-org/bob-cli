//! Seam-consistency tests: the narrow `index`-phase seams in
//! `highlights_ref` agree with the canonical mappings they bridge.

#[test]
fn ref_task_mark_seam_matches_the_pdf_task_mapping() {
    for (mark, status) in [
        (' ', "ready"),
        ('*', "next"),
        ('/', "wip"),
        ('x', "read"),
        ('X', "read"),
        ('-', "abandoned"),
    ] {
        let task = super::PdfTaskLine {
            line_index: 0,
            checkbox_mark_index: 0,
            checked: false,
            mark,
        };
        assert_eq!(
            super::ref_task_mark_status(mark),
            task.status().target_status(),
            "seam disagrees for [{mark}]"
        );
        assert_eq!(
            super::ref_task_mark_status(mark),
            Some(status),
            "wrong canonical status for [{mark}]"
        );
    }
    for mark in ['?', '!', 'o', 'c'] {
        assert_eq!(
            super::ref_task_mark_status(mark),
            None,
            "[{mark}] must stay unknown"
        );
    }
}

#[test]
fn deprecated_status_seam_matches_marker_normalization() {
    for (raw, normalized) in [
        ("unread", "ready"),
        ("done", "read"),
        ("ready", "ready"),
        ("read", "read"),
        ("legacy", "legacy"),
        ("wip", "wip"),
    ] {
        let mut projection = super::Projection::from([(
            "status".to_string(),
            super::MarkerValue::String(raw.to_string()),
        )]);
        super::normalize_deprecated_status(&mut projection);
        let canonical = projection
            .get("status")
            .and_then(super::MarkerValue::as_string)
            .expect("status survives normalization");
        assert_eq!(canonical, normalized, "canonical drift for {raw:?}");
        assert_eq!(
            super::normalize_deprecated_status_str(raw),
            normalized,
            "seam drift for {raw:?}"
        );
    }
}
