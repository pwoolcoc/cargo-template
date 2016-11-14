#![feature(proc_macro)]
#![recursion_limit = "1024"]

#[macro_use] extern crate log;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate error_chain;
extern crate cargo;
extern crate git2;
extern crate serde_json;
extern crate clap;

mod errors;

use std::env;
use std::path::{Path, PathBuf};
use std::fs::{File, DirBuilder, self};
use std::collections::HashMap;

use cargo::util;
use git2::Repository;
use clap::{App, Arg, ArgSettings};

use errors::*;

const DEFAULT_INDEX: &'static str = "https://github.com/rusttemplates/templates";

fn ensure_exists<P: AsRef<Path>>(p: P) -> Result<()> {
        let p = p.as_ref();
        let _ = DirBuilder::new().recursive(true).create(p)?;
        Ok(())
}


pub struct Config {
    pub index: String,
    pub index_path: PathBuf,
    pub templates_path: PathBuf,
    pub resolved_index_path: Option<PathBuf>,
}

impl Config {
    fn new() -> Result<Config> {
        let cargo_config = util::Config::default()?;
        let index = cargo_config.get_string("template.registry.index")?;
        let index = match index {
            Some(ref val) if val.val.len() == 0 => DEFAULT_INDEX.to_string(),
            None => DEFAULT_INDEX.to_string(),
            Some(val) => {
                val.val.to_string()
            },
        };

        let config_dir = Path::new(&env::var("CARGO_HOME")?).join("cargo-template");
        ensure_exists(&config_dir)?;

        let index_path = config_dir.join("index");
        ensure_exists(&index_path)?;

        let templates_path = config_dir.join("templates");
        ensure_exists(&templates_path)?;

        Ok(Config {
            index: index,
            index_path: index_path,
            templates_path: templates_path,
            resolved_index_path: None,
        })
    }
}

#[derive(Deserialize, Debug)]
struct IndexMember {
    name: String,
    loc: String,
}

#[derive(Deserialize, Debug)]
struct IndexTopLevel {
    index: Vec<IndexMember>,
}

impl IntoIterator for IndexTopLevel {
    type Item = (String, String);
    type IntoIter = IndexIter;

    fn into_iter(self) -> IndexIter {
        IndexIter {
            next: 0,
            inner: self,
        }
    }
}

struct IndexIter {
    next: usize,
    inner: IndexTopLevel,
}

impl Iterator for IndexIter {
    type Item = (String, String);

    fn next(&mut self) -> Option<Self::Item> {
        let el = self.inner.index.get(self.next).map(|el| (el.name.clone(), el.loc.clone()));
        self.next += 1;
        el
    }
}

struct IndexLoader<'a> {
    index: &'a Path,
}

impl<'a> IndexLoader<'a> {
    fn new(index: &'a Path) -> IndexLoader<'a> {
        IndexLoader {
            index: index,
        }
    }

    fn update_or_clone(&self, source: &str, frozen: bool) -> Result<PathBuf> {
        let repo = self.index.join(self.url_to_repo_dir(source));
        if repo.exists()  && repo.is_dir() {
            if !frozen {
                // self.update_index(source)
            }
            Ok(repo)
        } else {
            self.clone_index(source)
        }
    }

    fn update_index(&self, _source: &str) -> Result<PathBuf> {
        Err(ErrorKind::GenericError.into())
    }

    fn clone_index(&self, source: &str) -> Result<PathBuf> {
        // hacky and not-sufficient way to turn a url into a valid (single) directory name
        let p = self.index.join(self.url_to_repo_dir(source));
        let _ = Repository::clone(source, &p)?;
        debug!("cloned index at {:?}", &p);
        Ok(p)
    }

    fn url_to_repo_dir(&self, url: &str) -> String {
        url.replace(':', "_").replace('/', "_").replace(' ', "-")
    }
}

fn get_index(config: &mut Config, frozen: bool) -> Result<HashMap<String, String>> {
    let i = IndexLoader::new(&config.index_path);
    if let Ok(p) = i.update_or_clone(&config.index, frozen) {
        config.resolved_index_path = Some(p);
    }
    let index_file = match config.resolved_index_path {
        Some(ref p) => p.join("index.json"),
        None => {
            error!("Could not find an index");
            return Err(ErrorKind::GenericError.into());
        }
    };
    debug!("looking for index file {:?}", index_file);
    let index_file = File::open(index_file)?;
    let index_members = serde_json::from_reader::<File, IndexTopLevel>(index_file)?;
    let index_members: HashMap<String, String> = index_members.into_iter().collect();
    Ok(index_members)
}

fn get_template<P: AsRef<Path>>(name: &str, url: &str, templates_dir: P, frozen: bool) -> Result<PathBuf> {
    let templates_dir = templates_dir.as_ref();
    let location = templates_dir.join(name);
    if !location.exists() {
        if frozen {
            return Err(ErrorKind::TemplateNotFound(name.into()).into())
        }
        let _ = Repository::clone(url, &location);
    }

    Ok(location)
}

fn copy_dir<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();

    debug!("CALLED copy_dir WITH {}, {}", from.to_str().unwrap(), to.to_str().unwrap());

    if !from.exists() || !from.is_dir() {
        return Err(ErrorKind::SourceDoesNotExist(from.to_string_lossy().into_owned()).into());
    }
    ensure_exists(to)?;

    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let lossy = file_name.to_string_lossy();
        if lossy == ".git" {
            continue;
        }
        let path = entry.path();

        if path.is_dir() {
            let new_to = to.join(&file_name);
            ensure_exists(&new_to)?;
            debug!("from {} to {}", path.to_str().unwrap(), new_to.to_str().unwrap());
            copy_dir(path, new_to)?;
        } else if path.is_file() {
            let new_to = to.join(&file_name);
            debug!("copy {:?} to {:?}", path, new_to);
            fs::copy(&path, &new_to)?;
        }
    }
    Ok(())
}

fn cli() -> App<'static, 'static> {
    App::new("cargo-template")
        .about("initialize new cargo projects from a predefined template")
        .arg(Arg::with_name("frozen")
                .long("frozen")
                .help("Asserts that we shouldn't touch the network"))
        .arg(Arg::with_name("CARGO_ADDS_THIS")
                .set(ArgSettings::Hidden)
                .required(true)
                .index(1))
        .arg(Arg::with_name("TEMPLATE")
                .help("The template to use")
                .required(true)
                .index(2))
        .arg(Arg::with_name("NAME")
                .help("the project name")
                .required(true)
                .index(3))
}

pub fn main() -> Result<()> {
    let matches = cli().get_matches();
    let frozen = matches.is_present("frozen");
    let template = matches.value_of("TEMPLATE").unwrap(); // If we've gotten here, clap has verified that we have this
    let project_name = matches.value_of("NAME").unwrap();
    debug!("template: {:?}", template);
    debug!("project name: {:?}", project_name);
    let mut config = Config::new()?;
    let index = get_index(&mut config, frozen)?;
    let location = match index.get(template) {
        Some(loc) => loc,
        None => return Err(ErrorKind::TemplateDoesNotExist(template.into()).into())
    };
    debug!("template location is {:?}", location);
    let from = match get_template(template, location, &config.templates_path, frozen) {
        Ok(loc) => loc,
        Err(e) => {
            error!("Error getting template: {}", e);
            return Err(e);
        }
    };
    let cwd = env::current_dir()?;
    let project_dir = cwd.join(project_name);
    debug!("creating project at {:?}", project_dir);
    copy_dir(&from, &project_dir)?;
    debug!("substituting name & author values");
    // open new Cargo.toml && change the name & author lines
    
    Ok(())
}