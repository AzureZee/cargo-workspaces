use crate::utils::{Error, Result, git, info, warn};

use camino::Utf8PathBuf;
use cargo_metadata::MetadataCommand;
use clap::{ArgEnum, Parser};
use dunce::canonicalize;
use glob::glob;
use toml_edit::{Array, Document, Formatted, Item, Table, Value};

use std::{
    collections::HashSet, env, fs::{self, read_to_string, write}, io::ErrorKind, path::PathBuf
};

#[derive(Debug, Clone, Copy, ArgEnum)]
pub enum Resolver {
    #[clap(name = "1")]
    V1,
    #[clap(name = "2")]
    V2,
    #[clap(name = "3")]
    V3,
}

impl Resolver {
    fn name(&self) -> &str {
        match self {
            Resolver::V1 => "1",
            Resolver::V2 => "2",
            Resolver::V3 => "3",
        }
    }
}

/// Initializes a new cargo workspace
#[derive(Debug, Parser)]
pub struct Init {
    /// Path to the workspace root
    #[clap(parse(from_os_str), default_value = ".")]
    pub path: PathBuf,

    /// Workspace feature resolver version
    /// [default: 3]
    #[clap(short, long, arg_enum)]
    pub resolver: Option<Resolver>,
}

impl Init {
    pub fn run(&self) -> Result {
        // Create directory if it doesn't exist
        if !self.path.is_dir() {
            self.new_ws_repo()?
        }

        let cargo_toml = self.path.join("Cargo.toml");

        // NOTE: Globset is not used here because it does not support file iterator
        let pkgs = glob(&format!("{}/**/Cargo.toml", self.path.display()))?.filter_map(|e| e.ok());

        let mut workspace_roots = HashSet::new();

        for path in pkgs {
            let metadata = MetadataCommand::default()
                .manifest_path(path)
                .exec()
                .map_err(|e| Error::Init(e.to_string()))?;

            workspace_roots.insert(metadata.workspace_root);
        }

        let ws = canonicalize(&self.path)?;

        let mut document = match read_to_string(cargo_toml.as_path()) {
            Ok(manifest) => manifest.parse()?,
            Err(err) if err.kind() == ErrorKind::NotFound => Document::default(),
            Err(err) => return Err(err.into()),
        };

        let is_root_package = document.get("package").is_some();

        let workspace = document
            .entry("workspace")
            .or_insert_with(|| Item::Table(Table::default()))
            .as_table_mut()
            .ok_or_else(|| {
                Error::WorkspaceBadFormat(
                    "no workspace table found in workspace Cargo.toml".to_string(),
                )
            })?;

        // workspace members
        {
            let workspace_members = workspace
                .entry("members")
                .or_insert_with(|| Item::Value(Value::Array(Array::new())))
                .as_array_mut()
                .ok_or_else(|| {
                    Error::WorkspaceBadFormat(
                        "members was not an array in workspace Cargo.toml".to_string(),
                    )
                })?;

            if !workspace_members.is_empty() {
                info!("already initialized", self.path.display());
                return Ok(());
            }

            let mut members: Vec<_> = workspace_roots
                .iter()
                .filter_map(|m| m.strip_prefix(&ws).ok())
                .map(|path| path.to_string())
                .collect();

            // Remove the root Cargo.toml if not package
            if !is_root_package
                && let Some(index) = members.iter().position(|x| x.is_empty()) {
                    members.remove(index);
                }

            members.sort();

            info!("crates", members.join(", "));

            let max_member = members.len().saturating_sub(1);

            workspace_members.extend(members.into_iter().enumerate().map(|(i, val)| {
                let prefix = "\n    ";
                let suffix = if i == max_member { ",\n" } else { "" };
                Value::String(Formatted::new(val)).decorated(prefix, suffix)
            }));
        }

        // workspace resolver
        if let Some(resolver) = self.resolver.or(Some(Resolver::V3)) {
            workspace.entry("resolver").or_insert_with(|| {
                Item::Value(Value::String(Formatted::new(resolver.name().to_owned())))
            });
        }

        write(cargo_toml, document.to_string())?;

        info!("initialized", self.path.display());
        Ok(())
    }

    fn new_ws_repo(&self) -> Result {
        let current_dir = match env::current_dir() {
            Ok(dir) => dir,
            Err(get_current_dir_err) => return Err(Error::Io(get_current_dir_err)),
        };

        // Create absolute path by joining current directory with the provided path
        let new_dir = current_dir.join(&self.path);

        // Create directory if it doesn't exist
        if let Err(create_dir_err) = fs::create_dir_all(&new_dir) {
            return Err(Error::Io(create_dir_err));
        }

        // Run git init command
        let new_dir_utf8 = Utf8PathBuf::from_path_buf(new_dir.clone())
            .unwrap_or_else(|_| panic!("{} is not valid UTF-8.", &new_dir.display()));

        let (exit_status, ..) = git(&new_dir_utf8, &["init"])?;
        if !exit_status.success() {
            warn!("git repository init failed ", &new_dir.display());
        }

        // Create .gitignore file with content "/target"
        let gitignore_path = new_dir.join(".gitignore");

        Ok(if fs::write(&gitignore_path, "**/target").is_err() {
            warn!(
                "create or write .gitignore failed ",
                &gitignore_path.display()
            );
        })
    }
}
