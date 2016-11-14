use cargo::CargoError;
use std::env;
use git2;
use std::io;
use serde_json;

error_chain! { 
    foreign_links {
        Box<CargoError>, CargoKind;
        env::VarError, VarError;
        git2::Error, GitError;
        io::Error, IoError;
        serde_json::Error, SerdeError;
    }

    errors {
        GenericError
        TemplateDoesNotExist(t: String) {
            description("template not in index")
            display("Could not find template {} in the index", t)
        }
        TemplateNotFound(t: String) {
            description("template not found locally")
            display("Could not find template {} locally", t)
        }
        SourceDoesNotExist(t: String) {
            description("source directory error")
            display("Source directory '{}' does not exist or is not a directory", t)
        }
    }
}
