extern crate client;
extern crate futures;
extern crate tokio;

use futures::sync::mpsc;
use tokio::prelude::*;

use std::env;
use std::io;
use std::net::SocketAddr;
use std::thread;

fn main() -> Result<(), Box<std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();

    // Panic if no arguments are passed along to the program
    let addr = args
        .first()
        .unwrap_or_else(|| panic!("this program requires at least one argument"));

    // `addr` must be a valid `SocketAddr`
    let addr = addr.parse::<SocketAddr>()?;

    // This is Tokio's way of implementing a handle to stdin running on the
    // event loop.
    //
    // Read data with blocking I/O (from `read_stdin`) and send it to the
    // event loop over the standard futures channel.
    let (stdin_send, stdin_recv) = mpsc::channel(0);
    thread::spawn(|| client::read_stdin(stdin_send));
    let stdin_recv = stdin_recv.map_err(|_| panic!());

    // Connect to stream of bytes
    let stdout = client::connect(&addr, Box::new(stdin_recv));

    // With the stream of bytes, execute the write to stdout on the event loop
    let mut out = io::stdout();

    tokio::run({
        stdout
            .for_each(move |chunk| out.write_all(&chunk))
            .map_err(|e| println!("error reading stdout; error = {:?}", e))
    });
    Ok(())
}
