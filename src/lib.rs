#![feature(proc_macro)]
#![recursion_limit = "1024"]

#[macro_use] extern crate log;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate error_chain;
extern crate cargo;
extern crate git2;
extern crate serde_json;
extern crate clap;
extern crate toml;

mod errors;

use std::env;
use std::path::{Path, PathBuf};
use std::fs::{DirBuilder, File, OpenOptions, self};
use std::collections::HashMap;
use std::io::{Read, Write};

use cargo::util;
use git2::Repository;
use git2::Config as GitConfig;
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
                self.update_index(&repo)
            } else {
                Ok(repo)
            }
        } else {
            self.clone_index(source)
        }
    }

    fn update_index<P: AsRef<Path>>(&self, source: P) -> Result<PathBuf> {
        let source = source.as_ref();
        Ok(source.to_path_buf())
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

    debug!("copy_dir({}, {})", from.to_str().unwrap(), to.to_str().unwrap());

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
            debug!("copy {} to {}", path.to_str().unwrap(), new_to.to_str().unwrap());
            fs::copy(&path, &new_to)?;
        } else {
            error!("Oops, this isn't a directory or a file, I don't know how to handle this so I'm just gonna ignore it");
            error!("problem entry: {:?}", path.to_str());
        }
    }
    Ok(())
}

// Basically just a port of "get_environment_variable" from cargo/src/cargo/ops/cargo_new.rs
fn get_environment_variable(variables: &[&str]) -> Option<String> {
    variables.iter()
             .filter_map(|var| env::var(var).ok())
             .next()
}

// Basically just a port of "discover_author" from cargo/src/cargo/ops/cargo_new.rs
fn get_name_and_email() -> Result<(String, Option<String>)> {
    let git_config = GitConfig::open_default().ok();
    let git_config = git_config.as_ref();
    let name_variables = ["CARGO_NAME", "GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME",
                          "USER", "USERNAME", "NAME"];
    let name = get_environment_variable(&name_variables[0..3])
                    .or_else(|| git_config.and_then(|g| g.get_string("user.name").ok()))
                    .or_else(|| get_environment_variable(&name_variables[3..]));
    let name = match name {
        Some(name) => name,
        None => {
            let username_var = if cfg!(windows) { "USERNAME" } else { "USER" };
            return Err(ErrorKind::UserError(username_var.into()).into());
        }
    };
    let email_variables = ["CARGO_EMAIL", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL",
                           "EMAIL"];
    let email = get_environment_variable(&email_variables[0..3])
                    .or_else(|| git_config.and_then(|g| g.get_string("user.email").ok()))
                    .or_else(|| get_environment_variable(&email_variables[3..]));
    let name = name.trim().to_string();
    let email = email.map(|s| s.trim().to_string());

    Ok((name, email))
}

fn format_author(author_name: &str, author_email: &Option<String>) -> String {
    match *author_email {
        Some(ref email) => format!("{} <{}>", author_name, email),
        None => format!("{}", author_name),
    }
}

fn write_toml<P: AsRef<Path>>(file: P, val: toml::Value) -> Result<()> {
    let file = file.as_ref();
    let mut file = OpenOptions::new().read(true).write(true).open(file)?;
    let contents = format!("{}", val);
    write!(file, "{}", contents)?;
    Ok(())
}
fn edit_cargo_toml<P: AsRef<Path>>(file: P, project_name: &str, author_name: &str,
                                   author_email: &Option<String>) -> Result<()> {
    let file = file.as_ref();
    let mut contents = String::new();
    File::open(file)?.read_to_string(&mut contents)?;
    let contents = contents;
    let mut parser = toml::Parser::new(&contents);
    let mut value = match parser.parse() {
        Some(val) => val,
        None => return Err(ErrorKind::TomlParseError(file.to_string_lossy().into_owned()).into()),
    };
    match value.get_mut("package") {
        Some(&mut toml::Value::Table(ref mut t)) => {
            t.insert("name".into(), toml::Value::String(project_name.to_string()));
        },
        Some(_) => return Err(ErrorKind::TomlParseError(file.to_string_lossy().into_owned()).into()),
        None => return Err(ErrorKind::TomlParseError(file.to_string_lossy().into_owned()).into()),
    };

    match value.get_mut("package") {
        Some(&mut toml::Value::Table(ref mut t)) => {
            t.insert("authors".into(), 
                     toml::Value::Array(
                         vec![toml::Value::String(format_author(author_name, author_email))]));
        },
        Some(_) => return Err(ErrorKind::TomlParseError(file.to_string_lossy().into_owned()).into()),
        None => return Err(ErrorKind::TomlParseError(file.to_string_lossy().into_owned()).into()),
    }

    write_toml(&file, toml::Value::Table(value))?;

    Ok(())
}
fn find_cargo_toml<P: AsRef<Path>>(project_dir: P, project_name: &str,
                                      author_name: &str, author_email: &Option<String>) -> Result<()> {
    let project_dir = project_dir.as_ref();
    for entry in fs::read_dir(project_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && entry.file_name().to_string_lossy() == "Cargo.toml" {
            edit_cargo_toml(&path, project_name, author_name, author_email)?;
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
    let cwd = env::current_dir()?;
    let project_dir = cwd.join(project_name);
    if project_dir.exists() {
        return Err(ErrorKind::ExistsError(project_dir.to_string_lossy().into_owned()).into());
    }
    debug!("template: {:?}", template);
    debug!("project name: {:?}", project_name);
    let mut config = Config::new()?;
    let metadata = fs::metadata(template);
    let from = if metadata.is_ok() && metadata.unwrap().is_dir() {
        debug!("found template on filesystem");
        Path::new(template).to_path_buf()
    } else {
        let index = get_index(&mut config, frozen)?;
        let location = match index.get(template) {
            Some(loc) => loc,
            None => return Err(ErrorKind::TemplateDoesNotExist(template.into()).into())
        };
        debug!("template url is {:?}", location);
        let from = match get_template(template, location, &config.templates_path, frozen) {
            Ok(loc) => loc,
            Err(e) => {
                error!("Error getting template: {}", e);
                return Err(e);
            }
        };
        from
    };
    debug!("creating project at {:?}", project_dir);
    copy_dir(&from, &project_dir)?;
    debug!("substituting name & author values");
    // open new Cargo.toml && change the name & author lines
    let (author_name, author_email) = get_name_and_email()?;
    debug!("using author info `({:?}, {:?})`", author_name, author_email);
    find_cargo_toml(&project_dir, &project_name, &author_name, &author_email)?;
    Ok(())
}