//! JSON writer for the catalog document.

use serde::Serialize;

use super::assemble::{
    Alias, Arg, Catalog, CatalogSection, Entry, ExcludedPath, ExitCode,
};

#[derive(Serialize)]
struct Document<'a> {
    schema_version: u32,
    ok: bool,
    protocol: u32,
    bob_version: &'a str,
    features: &'a [String],
    sections: Vec<SectionJson<'a>>,
    roots: &'a [String],
    aliases: Vec<AliasJson<'a>>,
    entries: Vec<EntryJson<'a>>,
    excluded: Vec<ExcludedJson<'a>>,
}

#[derive(Serialize)]
struct SectionJson<'a> {
    id: &'a str,
    title: &'a str,
}

#[derive(Serialize)]
struct AliasJson<'a> {
    from: &'a [String],
    to: &'a [String],
}

#[derive(Serialize)]
struct EntryJson<'a> {
    id: &'a str,
    path: &'a [String],
    fixed: &'a [String],
    title: &'a str,
    about: &'a str,
    section: &'a str,
    keywords: &'a [String],
    effect: &'a str,
    network: bool,
    presentation: &'a str,
    policy: &'a str,
    preview: Option<&'a [String]>,
    confirm: Option<&'a str>,
    budget_secs: u32,
    args: Vec<ArgJson<'a>>,
    exit_codes: Vec<ExitJson<'a>>,
    adapter: Option<&'a str>,
    hosts: &'a [String],
    invalidates: &'a [String],
}

#[derive(Serialize)]
struct ArgJson<'a> {
    name: &'a str,
    slot: &'a str,
    required: bool,
    repeat: bool,
}

#[derive(Serialize)]
struct ExitJson<'a> {
    code: i32,
    outcome: &'a str,
    meaning: &'a str,
}

#[derive(Serialize)]
struct ExcludedJson<'a> {
    path: &'a [String],
    reason: &'a str,
}

pub(crate) fn render(catalog: &Catalog) -> Result<String, serde_json::Error> {
    serde_json::to_string(&Document {
        schema_version: catalog.schema_version,
        ok: catalog.ok,
        protocol: catalog.protocol,
        bob_version: &catalog.bob_version,
        features: &catalog.features,
        sections: catalog.sections.iter().map(section_json).collect(),
        roots: &catalog.roots,
        aliases: catalog.aliases.iter().map(alias_json).collect(),
        entries: catalog.entries.iter().map(entry_json).collect(),
        excluded: catalog.excluded.iter().map(excluded_json).collect(),
    })
}

fn section_json(section: &CatalogSection) -> SectionJson<'_> {
    SectionJson {
        id: &section.id,
        title: &section.title,
    }
}

fn alias_json(alias: &Alias) -> AliasJson<'_> {
    AliasJson {
        from: &alias.from,
        to: &alias.to,
    }
}

fn entry_json(entry: &Entry) -> EntryJson<'_> {
    EntryJson {
        id: &entry.id,
        path: &entry.path,
        fixed: &entry.fixed,
        title: &entry.title,
        about: &entry.about,
        section: &entry.section,
        keywords: &entry.keywords,
        effect: &entry.effect,
        network: entry.network,
        presentation: &entry.presentation,
        policy: &entry.policy,
        preview: entry.preview.as_deref(),
        confirm: entry.confirm.as_deref(),
        budget_secs: entry.budget_secs,
        args: entry.args.iter().map(arg_json).collect(),
        exit_codes: entry.exit_codes.iter().map(exit_json).collect(),
        adapter: entry.adapter.as_deref(),
        hosts: &entry.hosts,
        invalidates: &entry.invalidates,
    }
}

fn arg_json(arg: &Arg) -> ArgJson<'_> {
    ArgJson {
        name: &arg.name,
        slot: &arg.slot,
        required: arg.required,
        repeat: arg.repeat,
    }
}

fn exit_json(hint: &ExitCode) -> ExitJson<'_> {
    ExitJson {
        code: hint.code,
        outcome: &hint.outcome,
        meaning: &hint.meaning,
    }
}

fn excluded_json(row: &ExcludedPath) -> ExcludedJson<'_> {
    ExcludedJson {
        path: &row.path,
        reason: &row.reason,
    }
}
