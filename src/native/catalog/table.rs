//! Authored catalog rows. Derived fields (about, positional names,
//! required/repeat, aliases, roots, sections) come from the clap tree
//! and `SUBCOMMANDS` at assemble time. This table authors everything
//! else, including each positional's `slot` (default `text`).

/// Known argument slots. Unknown slots fail the coverage tests; clients
/// treat an unknown slot as `text`.
#[cfg(test)]
pub(crate) const KNOWN_SLOTS: &[&str] = &[
    "area-note",
    "article-note",
    "metric",
    "pdf",
    "ref",
    "ref-query",
    "ref-target",
    "relation-label",
    "text",
    "theme",
    "url",
];

/// Cache keys a finished run may invalidate. Unknown values fail the
/// coverage tests.
#[cfg(test)]
pub(crate) const KNOWN_INVALIDATES: &[&str] = &[
    "plugins",
    "references",
    "relations",
    "stats",
    "tasks",
    "themes",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    Reads,
    Writes,
    Syncs,
}

impl Effect {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reads => "reads",
            Self::Writes => "writes",
            Self::Syncs => "syncs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presentation {
    Report,
    Action,
    Inline,
}

impl Presentation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Action => "action",
            Self::Inline => "inline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Policy {
    Run,
    PreviewFirst,
    Confirm,
}

impl Policy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::PreviewFirst => "preview-first",
            Self::Confirm => "confirm",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StaticArg {
    pub name: &'static str,
    pub slot: &'static str,
    pub required: bool,
    pub repeat: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExitHint {
    pub code: i32,
    pub outcome: &'static str,
    pub meaning: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Row {
    pub id: &'static str,
    pub path: &'static [&'static str],
    pub fixed: &'static [&'static str],
    pub title: &'static str,
    pub keywords: &'static [&'static str],
    pub effect: Effect,
    pub network: bool,
    pub presentation: Presentation,
    pub policy: Policy,
    pub preview: Option<&'static [&'static str]>,
    pub confirm: Option<&'static str>,
    pub budget_secs: u32,
    pub slots: &'static [(&'static str, &'static str)],
    pub extra_args: &'static [StaticArg],
    pub exit_codes: &'static [ExitHint],
    pub adapter: Option<&'static str>,
    pub hosts: &'static [&'static str],
    pub invalidates: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Excluded {
    pub path: &'static [&'static str],
    pub reason: &'static str,
}

const DRY_RUN: &[&str] = &["--dry-run"];
const JSON: &[&str] = &["-f", "json"];

const fn reads(
    id: &'static str,
    path: &'static [&'static str],
    title: &'static str,
    keywords: &'static [&'static str],
    presentation: Presentation,
    budget_secs: u32,
) -> Row {
    Row {
        id,
        path,
        fixed: &[],
        title,
        keywords,
        effect: Effect::Reads,
        network: false,
        presentation,
        policy: Policy::Run,
        preview: None,
        confirm: None,
        budget_secs,
        slots: &[],
        extra_args: &[],
        exit_codes: &[],
        adapter: None,
        hosts: &[],
        invalidates: &[],
    }
}

const fn writes(
    id: &'static str,
    path: &'static [&'static str],
    title: &'static str,
    keywords: &'static [&'static str],
    policy: Policy,
    preview: Option<&'static [&'static str]>,
    budget_secs: u32,
) -> Row {
    Row {
        id,
        path,
        fixed: &[],
        title,
        keywords,
        effect: Effect::Writes,
        network: false,
        presentation: Presentation::Action,
        policy,
        preview,
        confirm: None,
        budget_secs,
        slots: &[],
        extra_args: &[],
        exit_codes: &[],
        adapter: None,
        hosts: &[],
        invalidates: &[],
    }
}

const fn syncs(
    id: &'static str,
    path: &'static [&'static str],
    title: &'static str,
    keywords: &'static [&'static str],
    policy: Policy,
    preview: Option<&'static [&'static str]>,
    budget_secs: u32,
) -> Row {
    Row {
        id,
        path,
        fixed: &[],
        title,
        keywords,
        effect: Effect::Syncs,
        network: false,
        presentation: Presentation::Action,
        policy,
        preview,
        confirm: None,
        budget_secs,
        slots: &[],
        extra_args: &[],
        exit_codes: &[],
        adapter: None,
        hosts: &[],
        invalidates: &[],
    }
}

/// Every useful command. Pomodoro session rows ship because
/// `pomodoro_actions` is yes. Free-text capture stays excluded.
pub(crate) const ENTRIES: &[Row] = &[
    // Daily workflow
    reads(
        "plan",
        &["plan"],
        "Today's plan",
        &["plan", "today", "budget", "lanes", "agenda"],
        Presentation::Report,
        60,
    ),
    reads(
        "freshness",
        &["freshness", "list"],
        "Review queue",
        &["freshness", "review", "walk", "queue", "rotten", "new"],
        Presentation::Report,
        60,
    ),
    Row {
        slots: &[("NOTE", "area-note"), ("note", "area-note")],
        ..reads(
            "ready",
            &["ready"],
            "Ready lanes",
            &["ready", "cap", "lane", "note"],
            Presentation::Report,
            60,
        )
    },
    Row {
        adapter: Some("pomodoro-status"),
        ..reads(
            "pomodoro-status",
            &["pomodoro", "status"],
            "Pomodoro status",
            &["pom", "tomato", "pomodoro", "status", "timer", "session"],
            Presentation::Inline,
            30,
        )
    },
    Row {
        fixed: &["="],
        adapter: Some("capture-session"),
        invalidates: &["stats", "tasks"],
        ..writes(
            "pomodoro-start",
            &["capture"],
            "Start next Pomodoro",
            &["pom", "tomato", "pomodoro", "start", "session", "timer"],
            Policy::Run,
            Some(DRY_RUN),
            60,
        )
    },
    Row {
        fixed: &["x"],
        adapter: Some("capture-session"),
        invalidates: &["stats", "tasks"],
        ..writes(
            "pomodoro-stop",
            &["capture"],
            "Stop running Pomodoro",
            &["pom", "tomato", "pomodoro", "stop", "finish", "session"],
            Policy::Run,
            Some(DRY_RUN),
            60,
        )
    },
    Row {
        fixed: &["+"],
        adapter: Some("capture-session"),
        invalidates: &["stats"],
        ..writes(
            "pomodoro-extend",
            &["capture"],
            "Extend Pomodoro 5 min",
            &["pom", "tomato", "pomodoro", "extend", "plus", "longer"],
            Policy::Run,
            Some(DRY_RUN),
            60,
        )
    },
    Row {
        fixed: &["--", "-"],
        adapter: Some("capture-session"),
        invalidates: &["stats"],
        ..writes(
            "pomodoro-shorten",
            &["capture"],
            "Shorten Pomodoro 5 min",
            &["pom", "tomato", "pomodoro", "shorten", "minus", "shorter"],
            Policy::Run,
            Some(DRY_RUN),
            60,
        )
    },
    reads(
        "stats",
        &["stats", "overview"],
        "Stats glance",
        &["stats", "glance", "overview", "metrics", "worked"],
        Presentation::Report,
        60,
    ),
    Row {
        slots: &[("METRIC", "metric"), ("metrics", "metric")],
        ..reads(
            "stats-show",
            &["stats", "show"],
            "Stats chart",
            &["stats", "chart", "metric", "show", "detail"],
            Presentation::Report,
            60,
        )
    },
    reads(
        "stats-metrics",
        &["stats", "metrics"],
        "Stats metric catalog",
        &["stats", "metrics", "catalog", "ids"],
        Presentation::Report,
        60,
    ),
    reads(
        "themes-list",
        &["themes", "list"],
        "List Pomodoro themes",
        &["themes", "list", "pomodoro", "colors"],
        Presentation::Report,
        60,
    ),
    Row {
        fixed: JSON,
        slots: &[("NAME", "theme"), ("name", "theme")],
        adapter: Some("json-write"),
        invalidates: &["themes"],
        ..writes(
            "themes-hide",
            &["themes", "hide"],
            "Hide themes",
            &["themes", "hide", "recent"],
            Policy::Run,
            None,
            60,
        )
    },
    Row {
        fixed: JSON,
        slots: &[("NAME", "theme"), ("name", "theme")],
        adapter: Some("json-write"),
        invalidates: &["themes"],
        ..writes(
            "themes-unhide",
            &["themes", "unhide"],
            "Unhide themes",
            &["themes", "unhide", "show", "recent"],
            Policy::Run,
            None,
            60,
        )
    },
    Row {
        fixed: JSON,
        slots: &[("NAME", "theme"), ("name", "theme")],
        adapter: Some("json-write"),
        invalidates: &["themes"],
        ..writes(
            "themes-color",
            &["themes", "color"],
            "Pin a theme's hue",
            &["themes", "color", "hue", "pin"],
            Policy::Run,
            None,
            60,
        )
    },
    Row {
        slots: &[("OLD", "theme"), ("old", "theme")],
        invalidates: &["themes"],
        ..syncs(
            "themes-rename",
            &["themes", "rename"],
            "Rename a theme",
            &["themes", "rename", "rewrite"],
            Policy::PreviewFirst,
            Some(DRY_RUN),
            180,
        )
    },
    // Tasks and projects
    reads(
        "projects-list",
        &["projects", "list"],
        "Projects",
        &["projects", "list", "prj"],
        Presentation::Report,
        60,
    ),
    Row {
        invalidates: &["tasks"],
        ..writes(
            "projects-sync",
            &["projects", "sync"],
            "Sync projects",
            &["projects", "sync", "prj", "subprojects"],
            Policy::Run,
            Some(DRY_RUN),
            180,
        )
    },
    reads(
        "relations-list",
        &["relations", "list"],
        "Relation labels",
        &["relations", "labels", "links", "list"],
        Presentation::Report,
        60,
    ),
    Row {
        fixed: JSON,
        slots: &[("LABEL", "relation-label"), ("label", "relation-label")],
        adapter: Some("json-write"),
        invalidates: &["relations"],
        ..writes(
            "relations-hide",
            &["relations", "hide"],
            "Hide relation labels",
            &["relations", "hide", "labels"],
            Policy::Run,
            None,
            60,
        )
    },
    Row {
        fixed: JSON,
        slots: &[("LABEL", "relation-label"), ("label", "relation-label")],
        adapter: Some("json-write"),
        invalidates: &["relations"],
        ..writes(
            "relations-unhide",
            &["relations", "unhide"],
            "Unhide relation labels",
            &["relations", "unhide", "labels", "show"],
            Policy::Run,
            None,
            60,
        )
    },
    Row {
        slots: &[("OLD", "relation-label"), ("old", "relation-label")],
        invalidates: &["relations"],
        ..syncs(
            "relations-rename",
            &["relations", "rename"],
            "Rename a relation label",
            &["relations", "rename", "labels", "rewrite"],
            Policy::PreviewFirst,
            Some(DRY_RUN),
            180,
        )
    },
    Row {
        invalidates: &["tasks"],
        ..writes(
            "task-reconcile",
            &["task", "reconcile"],
            "Reconcile task statuses",
            &["reconcile", "status", "hooks", "lanes", "pending", "next"],
            Policy::Run,
            Some(DRY_RUN),
            180,
        )
    },
    Row {
        adapter: Some("task-reroll"),
        invalidates: &["tasks"],
        ..writes(
            "task-reroll",
            &["task", "reroll"],
            "Re-roll due prioritized tasks",
            &["reroll", "randomize", "priority", "scheduled"],
            Policy::PreviewFirst,
            Some(DRY_RUN),
            180,
        )
    },
    Row {
        confirm: Some(
            "Archive done and canceled tasks into done/ and repair vault \
             links. This command has no dry run.",
        ),
        invalidates: &["tasks"],
        ..writes(
            "task-archive",
            &["task", "archive"],
            "Archive done tasks",
            &["archive", "done", "collect", "move-done"],
            Policy::Confirm,
            None,
            300,
        )
    },
    // Vault
    reads(
        "articles-list",
        &["articles", "list"],
        "Articles",
        &["articles", "list", "notes"],
        Presentation::Report,
        60,
    ),
    Row {
        slots: &[("NOTE", "article-note"), ("note", "article-note")],
        ..reads(
            "articles-outline",
            &["articles", "outline"],
            "Article outline",
            &["articles", "outline", "sections"],
            Presentation::Report,
            60,
        )
    },
    Row {
        confirm: Some(
            "Run nightly maintenance (vault-sync, task archive, articles, \
             vault-sync). Athena's timer already runs this.",
        ),
        invalidates: &["tasks"],
        ..syncs(
            "nightly",
            &["nightly"],
            "Nightly maintenance",
            &["nightly", "maintenance", "cron", "timer"],
            Policy::Confirm,
            None,
            1800,
        )
    },
    Row {
        fixed: &["--source"],
        extra_args: &[StaticArg {
            name: "SOURCE",
            slot: "text",
            required: true,
            repeat: false,
        }],
        ..reads(
            "query-source",
            &["query"],
            "Find notes by Dataview source",
            &["query", "dataview", "source", "notes"],
            Presentation::Report,
            60,
        )
    },
    Row {
        fixed: &["--tasks"],
        extra_args: &[StaticArg {
            name: "QUERY",
            slot: "text",
            required: true,
            repeat: false,
        }],
        ..reads(
            "query-tasks",
            &["query"],
            "Run a Tasks query",
            &["query", "tasks", "filter"],
            Presentation::Report,
            60,
        )
    },
    Row {
        fixed: &["--query"],
        extra_args: &[StaticArg {
            name: "DQL",
            slot: "text",
            required: true,
            repeat: false,
        }],
        ..reads(
            "query-dql",
            &["query"],
            "Run a Dataview query",
            &["query", "dataview", "dql"],
            Presentation::Report,
            60,
        )
    },
    Row {
        slots: &[("TEXT", "url"), ("text", "url")],
        adapter: Some("ref-create"),
        invalidates: &["references", "tasks"],
        ..writes(
            "ref-queue",
            &["capture"],
            "Queue link for reading",
            &["reading", "queue", "url", "link", "inbox", "clip"],
            Policy::Run,
            Some(DRY_RUN),
            60,
        )
    },
    Row {
        slots: &[("TARGET", "ref-target"), ("target", "ref-target")],
        adapter: Some("ref-create"),
        network: true,
        invalidates: &["references", "tasks"],
        ..writes(
            "ref-create",
            &["ref", "create"],
            "Create reference",
            &[
                "pdf",
                "highlights",
                "clip",
                "snapshot",
                "url",
                "arxiv",
                "paper",
                "web",
                "create",
            ],
            Policy::Run,
            Some(DRY_RUN),
            900,
        )
    },
    Row {
        slots: &[("QUERY", "ref-query"), ("query", "ref-query")],
        ..reads(
            "ref-find",
            &["ref", "find"],
            "Find in library",
            &["find", "search", "library", "arxiv", "doi"],
            Presentation::Report,
            60,
        )
    },
    reads(
        "ref-list",
        &["ref", "list"],
        "Reading list",
        &["reading", "list", "library", "queue", "refs"],
        Presentation::Report,
        60,
    ),
    Row {
        slots: &[("REF", "ref"), ("ref", "ref")],
        ..reads(
            "ref-show",
            &["ref", "show"],
            "Show reference",
            &["show", "annotations", "notes", "comments"],
            Presentation::Report,
            60,
        )
    },
    Row {
        slots: &[("REF", "ref"), ("ref", "ref")],
        ..reads(
            "ref-code",
            &["ref", "code"],
            "Reference code blocks",
            &["code", "blocks", "snippet", "markdown", "source"],
            Presentation::Report,
            60,
        )
    },
    Row {
        slots: &[("REF", "ref"), ("ref", "ref")],
        ..reads(
            "ref-links",
            &["ref", "links"],
            "Reference links",
            &["links", "url", "markdown", "catalog"],
            Presentation::Report,
            60,
        )
    },
    Row {
        slots: &[("REF", "ref"), ("ref", "ref")],
        invalidates: &["tasks"],
        ..writes(
            "ref-today",
            &["ref", "today"],
            "Pull reading into today's Pomodoro",
            &["today", "reading", "pomodoro", "pull"],
            Policy::Run,
            Some(DRY_RUN),
            60,
        )
    },
    Row {
        slots: &[("REF", "ref"), ("ref", "ref")],
        invalidates: &["references", "tasks"],
        ..syncs(
            "ref-rename",
            &["ref", "rename"],
            "Rename a reference",
            &["rename", "reference", "stem"],
            Policy::PreviewFirst,
            Some(DRY_RUN),
            300,
        )
    },
    reads(
        "ref-doctor",
        &["ref", "doctor"],
        "Library health check",
        &["doctor", "health", "library", "highlights"],
        Presentation::Report,
        120,
    ),
    Row {
        adapter: Some("ref-scan"),
        invalidates: &["references", "tasks"],
        ..writes(
            "ref-scan",
            &["ref", "scan"],
            "Scan Highlights library",
            &[
                "pdf",
                "scan",
                "highlights",
                "library",
                "sync",
                "annotations",
            ],
            Policy::Run,
            Some(DRY_RUN),
            1800,
        )
    },
    Row {
        fixed: &["--intake-only"],
        adapter: Some("ref-scan"),
        invalidates: &["references", "tasks"],
        ..writes(
            "ref-scan-intake",
            &["ref", "scan"],
            "File queued intake",
            &["intake", "xlib", "queue", "file", "scan"],
            Policy::Run,
            Some(DRY_RUN),
            900,
        )
    },
    reads(
        "ref-jobs",
        &["ref", "jobs", "list"],
        "Reference jobs",
        &["jobs", "queue", "clip", "background"],
        Presentation::Report,
        60,
    ),
    Row {
        network: true,
        invalidates: &["references", "tasks"],
        ..writes(
            "ref-jobs-run",
            &["ref", "jobs", "run"],
            "Run queued reference jobs",
            &["jobs", "run", "clip", "queue", "worker"],
            Policy::Run,
            None,
            1800,
        )
    },
    Row {
        slots: &[("PDF", "pdf"), ("pdf", "pdf")],
        invalidates: &["references"],
        ..writes(
            "ref-sync",
            &["ref", "sync"],
            "Sync one PDF marker",
            &["sync", "pdf", "marker", "highlights"],
            Policy::Run,
            Some(DRY_RUN),
            300,
        )
    },
    Row {
        slots: &[("PDF", "pdf"), ("pdf", "pdf")],
        ..reads(
            "ref-marker",
            &["ref", "marker"],
            "Inspect a PDF marker",
            &["marker", "pdf", "inspect"],
            Presentation::Report,
            60,
        )
    },
    Row {
        adapter: Some("vault-sync"),
        ..syncs(
            "vault-sync",
            &["vault-sync", "run"],
            "Sync vault now",
            &[
                "push",
                "pull",
                "git",
                "commit",
                "sync",
                "vault",
                "reconcile",
            ],
            Policy::Run,
            Some(DRY_RUN),
            300,
        )
    },
    reads(
        "vault-sync-status",
        &["vault-sync", "status"],
        "Vault sync status",
        &["sync", "status", "git", "vault"],
        Presentation::Report,
        30,
    ),
    // Integrations
    Row {
        network: true,
        ..reads(
            "gkeep-list",
            &["gkeep", "list"],
            "Keep inbox",
            &["keep", "inbox", "google", "gkeep", "list"],
            Presentation::Report,
            60,
        )
    },
    Row {
        adapter: Some("gkeep-pull"),
        network: true,
        invalidates: &["tasks", "references"],
        ..writes(
            "gkeep-pull",
            &["gkeep", "pull"],
            "Drain Keep inbox",
            &["keep", "inbox", "google", "gkeep", "pull", "drain"],
            Policy::PreviewFirst,
            Some(DRY_RUN),
            300,
        )
    },
    Row {
        network: true,
        ..reads(
            "gkeep-doctor",
            &["gkeep", "doctor"],
            "Keep setup check",
            &["keep", "inbox", "google", "gkeep", "doctor", "setup"],
            Presentation::Report,
            60,
        )
    },
    // Setup
    Row {
        network: true,
        ..reads(
            "plugins-list",
            &["plugins", "list"],
            "Plugin versions",
            &["plugins", "list", "versions", "obsidian"],
            Presentation::Report,
            120,
        )
    },
    Row {
        fixed: JSON,
        adapter: Some("plugins-sync"),
        invalidates: &["plugins"],
        ..writes(
            "plugins-sync",
            &["plugins", "sync"],
            "Deploy plugins",
            &["deploy", "install", "plugins", "sync", "obsidian"],
            Policy::Run,
            Some(DRY_RUN),
            180,
        )
    },
];

/// Authored exclusions. Capture-protocol leaves and every `completion`
/// member are added automatically at assemble time.
pub(crate) const EXCLUDED: &[Excluded] = &[
    Excluded {
        path: &["capture"],
        reason: "Capture text from Bob Mac Capture or a terminal; command \
                 mode runs its named capture rows",
    },
    Excluded {
        path: &["pomodoro", "notify"],
        reason: "Long-running notifier that never exits",
    },
    Excluded {
        path: &["pomodoro", "tmux"],
        reason: "Prints a tmux status line",
    },
    Excluded {
        path: &["gkeep", "login"],
        reason: "Needs an interactive terminal and handles credentials",
    },
    Excluded {
        path: &["gkeep", "migrate-markers"],
        reason: "One-time migration; run it from a terminal",
    },
    Excluded {
        path: &["gkeep", "migrate-tasks"],
        reason: "One-time migration; run it from a terminal",
    },
    Excluded {
        path: &["ref", "attach-source"],
        reason: "Backfill that needs search paths; run it from a terminal",
    },
    Excluded {
        path: &["ref", "recatalog"],
        reason: "Saved-source catalog backfill; run it from a terminal",
    },
    Excluded {
        path: &["ref", "migrate-tasks"],
        reason: "One-time migration; run it from a terminal",
    },
    Excluded {
        path: &["ref", "migrate-zorg"],
        reason: "One-time migration; run it from a terminal",
    },
    Excluded {
        path: &["task", "move"],
        reason: "Use Ctrl+Shift+M, which moves tasks through the editor",
    },
    Excluded {
        path: &["articles", "format"],
        reason: "Use Format this article (bob-articles formats through the \
                 editor)",
    },
    Excluded {
        path: &["articles", "adopt"],
        reason: "Use bob-articles' adoption preview",
    },
];

pub(crate) const CAPTURE_PROTOCOL_REASON: &str =
    "Capture protocol for capture clients";
pub(crate) const COMPLETION_REASON: &str =
    "Shell completion; run it in a terminal";
