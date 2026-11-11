//! Discover `bank.md` and `bank/YYYY-MM.md`.

use std::{fs, path::Path};

use super::{grammar::YearMonth, month::filename_month_for};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HubFile {
    pub(crate) path: String,
    pub(crate) contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MonthFile {
    pub(crate) path: String,
    pub(crate) month: YearMonth,
    pub(crate) contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BankScan {
    pub(crate) hub: Option<HubFile>,
    pub(crate) bank_dir_exists: bool,
    pub(crate) months: Vec<MonthFile>,
    pub(crate) io_errors: Vec<String>,
}

pub(crate) fn scan_bank(bob_dir: &Path) -> BankScan {
    let mut io_errors = Vec::new();
    let hub = read_hub(bob_dir, &mut io_errors);
    let bank_dir = bob_dir.join("bank");
    let bank_dir_exists = bank_dir.is_dir();
    let months = if bank_dir_exists {
        read_months(&bank_dir, &mut io_errors)
    } else {
        Vec::new()
    };
    BankScan {
        hub,
        bank_dir_exists,
        months,
        io_errors,
    }
}

pub(crate) fn is_teaching_empty(scan: &BankScan) -> bool {
    scan.hub.is_none() && !scan.bank_dir_exists && scan.io_errors.is_empty()
}

fn read_hub(bob_dir: &Path, io_errors: &mut Vec<String>) -> Option<HubFile> {
    let path = bob_dir.join("bank.md");
    match fs::read_to_string(&path) {
        Ok(contents) => Some(HubFile {
            path: "bank.md".to_owned(),
            contents,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            io_errors.push(format!("bank.md: {error}"));
            None
        }
    }
}

fn read_months(bank_dir: &Path, io_errors: &mut Vec<String>) -> Vec<MonthFile> {
    let entries = match fs::read_dir(bank_dir) {
        Ok(entries) => entries,
        Err(error) => {
            io_errors.push(format!("bank/: {error}"));
            return Vec::new();
        }
    };
    let mut months = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                io_errors.push(format!("bank/: {error}"));
                continue;
            }
        };
        let path = entry.path();
        if !is_markdown(&path) {
            continue;
        }
        let Some(month) = filename_month_for(&path) else {
            continue;
        };
        match fs::read_to_string(&path) {
            Ok(contents) => months.push(MonthFile {
                path: format!("bank/{}.md", month.as_str()),
                month,
                contents,
            }),
            Err(error) => {
                io_errors.push(format!("bank/{}.md: {error}", month.as_str()));
            }
        }
    }
    months.sort_by(|left, right| left.path.cmp(&right.path));
    months
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("md")
}
