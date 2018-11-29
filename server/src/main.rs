extern crate client;
extern crate futures;
extern crate tokio;

use client::{Client, Shared};
use futures::future::Either;
use tokio::codec::{Framed, LengthDelimitedCodec};
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

use std::env;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

fn main() -> Result<(), Box<std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();

    // Panic if no arguments are passed along to the program
    let addr = args
        .first()
        .unwrap_or_else(|| panic!("this program requires at least one argument"));

    // `addr` must be a valid `SocketAddr`
    let addr = addr.parse::<SocketAddr>()?;

    // Create the TCP Listener to accept connections on `addr`
    let listener = TcpListener::bind(&addr)?;

    // Create a new state for all clients to share
    let state = Arc::new(Mutex::new(Shared::default()));

    let server = listener
        .incoming()
        .for_each(move |socket| {
            process(socket, state.clone());
            Ok(())
        }).map_err(|err| {
            println!("failed to accept socket; error = {}", err);
        });;

    println!("Server running on {}", addr);

    tokio::run(server);
    Ok(())
}

fn process(socket: TcpStream, state: Arc<Mutex<Shared>>) {
    let addr = socket.peer_addr().unwrap();
    let frames = Framed::new(socket, LengthDelimitedCodec::new());

    let connection = frames
        .into_future()
        .map_err(|(e, _)| e)
        .and_then(move |(name, frames)| {
            let name = match name {
                Some(name) => name,
                None => {
                    return Either::A(future::ok(()));
                }
            };

            println!("{:?} joined the chat", name);

            let client = Client::new(addr, frames, name, state);
            Either::B(client)
        }).map_err(|err| {
            println!("broken pipe; error = {:?}", err);
        });;

    tokio::spawn(connection);
}
