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
    // `[?]` (Blocked) has no fixed lifecycle status, so the fixed seam
    // stays `None`; its status-aware target is covered below.
    for mark in ['?', '!', 'o', 'c'] {
        assert_eq!(
            super::ref_task_mark_status(mark),
            None,
            "[{mark}] must stay unknown"
        );
    }
}

#[test]
fn ref_task_mark_target_seam_matches_status_aware_mapping() {
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
        for current in [None, Some("ready"), Some("read"), Some("legacy")] {
            assert_eq!(
                super::ref_task_mark_target_status(mark, current),
                task.status().target_status_given(current),
                "target seam disagrees for [{mark}] with {current:?}"
            );
        }
        assert_eq!(
            super::ref_task_mark_target_status(mark, None),
            Some(status),
            "wrong fixed target for [{mark}]"
        );
        assert!(
            super::is_known_ref_task_mark(mark),
            "[{mark}] must be known"
        );
    }
    assert!(super::is_known_ref_task_mark('?'), "[?] must be known");
    for open in ["ready", "next", "wip", "legacy"] {
        assert_eq!(
            super::ref_task_mark_target_status('?', Some(open)),
            Some(match open {
                "ready" => super::STATUS_READY,
                "next" => super::STATUS_NEXT,
                "wip" => super::STATUS_WIP,
                _ => super::STATUS_LEGACY,
            }),
            "[?] must agree with open {open}"
        );
    }
    for terminal in ["read", "abandoned"] {
        assert_eq!(
            super::ref_task_mark_target_status('?', Some(terminal)),
            Some(super::STATUS_READY),
            "[?] must reopen terminal {terminal} to ready"
        );
    }
    assert_eq!(
        super::ref_task_mark_target_status('?', None),
        None,
        "[?] without a current status has no target"
    );
    for mark in ['!', 'o', 'c', '>'] {
        assert!(
            !super::is_known_ref_task_mark(mark),
            "[{mark}] must be unknown"
        );
        assert_eq!(
            super::ref_task_mark_target_status(mark, Some("ready")),
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
