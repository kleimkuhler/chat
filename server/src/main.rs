extern crate bincode;
extern crate futures;
#[macro_use]
extern crate serde_derive;
extern crate tokio;

use bincode::{deserialize, serialize};
use tokio::io;
use tokio::net::TcpListener;
use tokio::prelude::*;

use std::collections::HashMap;
use std::io::BufReader;
use std::iter;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum MessageKind {
    Response,
    Text,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Message {
    kind: MessageKind,
    text: String,
    sender: SocketAddr,
    timestamp: SystemTime,
}

impl Message {
    fn new(kind: MessageKind, text: String, sender: SocketAddr, timestamp: SystemTime) -> Message {
        Message {
            kind,
            text,
            sender,
            timestamp,
        }
    }
}

fn main() {
    // Create the TCP listener to accept connections on
    let addr = "127.0.0.1:12345".parse::<SocketAddr>().unwrap();
    let listener = TcpListener::bind(&addr).unwrap();
    println!("Listening on: {}", addr);

    // Allow clients to be shared across threads
    let clients = Arc::new(Mutex::new(HashMap::new()));

    let server = listener
        .incoming()
        .map_err(|e| println!("failed to accept socket; error = {}", e))
        .for_each(move |stream| {
            // Get the socket address of the new client
            let addr = stream.peer_addr().unwrap();
            println!("New connection: {}", addr);

            // Split the <TcpStream> into reading and writing handles. This allows separate tasks
            // for each to be spawned on the event loop.
            let (reader, writer) = stream.split();

            // Create a channel for the new client and register it to the map of clients shared
            // across threads
            let (tx, rx) = futures::sync::mpsc::unbounded();
            clients.lock().unwrap().insert(addr, tx);

            // Buffer the reader and model the read portion of the socket with an infinite iterator
            let reader = BufReader::new(reader);
            let message_stream = stream::iter_ok::<_, io::Error>(iter::repeat(()));

            // Clone `clients` to prepare for moving into closure
            let clients_reader = clients.clone();

            // Whenever data is received on the transmitter, read it to `client_reader`
            let client_reader = message_stream.fold(reader, move |reader, _| {
                let line = io::read_until(reader, b'\n', Vec::new());
                let line = line.and_then(|(reader, data)| {
                    if data.len() != 0 {
                        Ok((reader, data))
                    } else {
                        Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
                    }
                });

                // Convert the bytes read into a <String>
                let line = line.map(|(reader, data)| (reader, String::from_utf8(data)));

                // Clone `clients_reader` to prepare for moving into closure
                let clients = clients_reader.clone();

                // Send a <Message> to each client on the server except the current one
                line.map(move |(reader, text)| {
                    let mut filtered_clients = clients.lock().unwrap();

                    // Create a <Message> with current client data and serialize it
                    if let Ok(txt) = text {
                        let message = Message::new(
                            MessageKind::Text,
                            txt.to_string(),
                            addr,
                            SystemTime::now(),
                        );
                        let encoded = serialize(&message).unwrap();

                        let iter = filtered_clients
                            .iter_mut()
                            .filter(|&(&k, _)| k != addr)
                            .map(|(_, v)| v);
                        for client in iter {
                            client.unbounded_send(encoded.clone()).unwrap();
                        }
                    }

                    reader
                })
            });

            // Clone `addr` and `clients` to prepare for moving into closure
            let clients_writer = clients.clone();

            // Whenever data is received on the Receiver, write it to the `client_writer`
            let client_writer = rx.fold(writer, move |writer, encoded| {
                // Deserialize the encoded mesage
                let decoded: Message = deserialize(&encoded).unwrap();

                if let MessageKind::Text = decoded.kind {
                    let clients = clients_writer.lock().unwrap();

                    // Create a <Message> to send acknowledgment back to sender
                    let response = Message::new(
                        MessageKind::Response,
                        "".to_string(),
                        addr,
                        decoded.timestamp,
                    );
                    let encoded = serialize(&response).unwrap();

                    let tx = clients.get(&decoded.sender).unwrap();
                    tx.unbounded_send(encoded.clone()).unwrap();
                }

                let message = match decoded.kind {
                    MessageKind::Response => {
                        let now = SystemTime::now();
                        let roundtrip = now.duration_since(decoded.timestamp).unwrap();
                        format!(
                            "Roundtrip time to {} was {{ {} secs {} ns}}\n",
                            decoded.sender,
                            roundtrip.as_secs(),
                            roundtrip.subsec_nanos()
                        )
                    }
                    MessageKind::Text => format!("{}: {}", decoded.sender, decoded.text),
                };
                let amt = io::write_all(writer, message);
                let amt = amt.map(|(writer, _)| writer);
                amt.map_err(|_| ())
            });

            // Use the `select` combinator to wait for either half of the client's socket to close
            // and then spawn the task
            let clients = clients.clone();
            let client_reader = client_reader.map_err(|_| ());
            let client = client_reader.map(|_| ()).select(client_writer.map(|_| ()));

            // Spawn a task to process the connection
            tokio::spawn(client.then(move |_| {
                clients.lock().unwrap().remove(&addr);
                println!("Connection {} closed.", addr);
                Ok(())
            }));

            Ok(())
        });

    tokio::run(server);
}
