#[macro_use] extern crate log;
extern crate env_logger;
extern crate template;

use std::process;
use std::error::Error;

fn main() {
    env_logger::init().unwrap();
    if let Err(e) = template::main() {
        error!("Got error {:?}", e.description());
        process::exit(1);
    }
}

