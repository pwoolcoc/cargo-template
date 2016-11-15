#[macro_use] extern crate log;
extern crate env_logger;
extern crate template;

use std::process;
use std::error::Error;
use std::io::{Write, stderr};

fn main() {
    env_logger::init().unwrap();
    if let Err(e) = template::main() {
        error!("Got error {:?}", e.description());
        let mut stderr = stderr();
        let _ = writeln!(stderr, "{}", e);
        process::exit(1);
    }
}

