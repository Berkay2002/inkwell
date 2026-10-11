//! Named sets of Advanced settings, kept in `<data dir>/compress-presets.json` (the same folder as the
//! recent files; `INKWELL_DATA_DIR` moves it).
//!
//! What is stored is the engine's own `Options`, so a preset saved today still loads after the engine
//! gains settings: fields the file lacks take their defaults, fields it has that this build does not
//! know are ignored. Never stored: the password and the output path (they are not part of the settings),
//! and the names of the fonts picked for unembedding (a document's own fonts; "unembed every font that
//! can be" is stored instead).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{validate, Error, Options, Preset};

pub const FILE: &str = "compress-presets.json";
/// More than this is a list nobody scrolls.
pub const MAX: usize = 50;
const NAME_MAX: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
    pub options: Options,
}

#[derive(Serialize, Deserialize)]
struct Disk {
    version: u32,
    presets: Vec<serde_json::Value>,
}

/// The options as a preset keeps them.
fn storable(o: &Options) -> Options {
    Options {
        preset: Preset::Custom,
        password: None,
        output: None,
        replace_output: false,
        unembed_fonts: Vec::new(),
        ..o.clone()
    }
}

fn clean_name(name: &str) -> Result<String, Error> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > NAME_MAX || n.chars().any(char::is_control) {
        return Err(Error::InvalidOptions(format!("a preset name is 1 to {NAME_MAX} characters")));
    }
    Ok(n.to_string())
}

/// Every preset the file holds, in the order they were saved. A missing or unreadable file is an
/// empty list; an entry that no longer validates is left out (the file keeps it until the next save).
pub fn load(dir: &Path) -> Vec<Named> {
    let Ok(text) = fs::read_to_string(dir.join(FILE)) else { return Vec::new() };
    let Ok(disk) = serde_json::from_str::<Disk>(&text) else { return Vec::new() };
    disk.presets
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Named>(v).ok())
        .filter(|n| clean_name(&n.name).is_ok() && validate(&n.options).is_ok())
        .map(|n| Named { options: storable(&n.options), ..n })
        .take(MAX)
        .collect()
}

fn write(dir: &Path, list: &[Named]) -> Result<(), Error> {
    fs::create_dir_all(dir)?;
    let disk = Disk {
        version: 1,
        presets: list.iter().map(|n| serde_json::to_value(n).expect("options serialize")).collect(),
    };
    // Write beside, then rename over: a crash never leaves half a file.
    let tmp = dir.join(format!("{FILE}.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(&disk).expect("options serialize"))?;
    fs::rename(&tmp, dir.join(FILE))?;
    Ok(())
}

/// Saves `options` under `name`, replacing a preset of the same name (any capitalisation). Returns
/// the list as it is now.
pub fn save(dir: &Path, name: &str, options: &Options) -> Result<Vec<Named>, Error> {
    let name = clean_name(name)?;
    let options = storable(options);
    validate(&options)?;
    let mut list = load(dir);
    match list.iter().position(|n| n.name.to_lowercase() == name.to_lowercase()) {
        Some(i) => list[i] = Named { name, options },
        None if list.len() >= MAX => return Err(Error::InvalidOptions(format!("at most {MAX} presets"))),
        None => list.push(Named { name, options }),
    }
    write(dir, &list)?;
    Ok(list)
}

pub fn delete(dir: &Path, name: &str) -> Result<Vec<Named>, Error> {
    let mut list = load(dir);
    let before = list.len();
    list.retain(|n| n.name.to_lowercase() != name.trim().to_lowercase());
    if list.len() != before {
        write(dir, &list)?;
    }
    Ok(list)
}
