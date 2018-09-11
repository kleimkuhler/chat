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
use std::env;
use std::io::BufReader;
use std::iter;
use std::mem;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// `MessageKind` is used to indicate if an incoming message is a `Response` or `Text` so that the
/// receiver can decide whether to create an acknowledgment or not.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum MessageKind {
    Response,
    Text,
}

/// `Message` is used to package the fields required for constructing an acknowledgment. This
/// involves the `kind`, `sender`, and `timestamp` fields.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Message {
    kind: MessageKind,
    text: String,
    sender: SocketAddr,
    timestamp: SystemTime,
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();

    // Panic if no arguments are passed along to the program
    let addr = args
        .first()
        .unwrap_or_else(|| panic!("this program requires at least one argument"));

    // Panic if the argument passed along is not <SocketAddr> format
    let addr = addr
        .parse::<SocketAddr>()
        .unwrap_or_else(|_| panic!("argument must be a valid <SocketAddr>"));

    // Create the TCP Listener to accept connections on `addr`
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

            // Buffer the reader. Model the read portion of the socket with an infinite iterator
            let reader = BufReader::new(reader);
            let message_stream = stream::iter_ok::<_, io::Error>(iter::repeat(()));

            // Clone `clients` to prepare for moving into closure
            let readers_clients = clients.clone();

            // Whenever data is received on the Sender, read it to `client_reader`
            let client_reader = message_stream.fold(reader, move |reader, _| {
                let line = io::read_until(reader, 0x04 as u8, Vec::new());

                let line = line.and_then(|(reader, data)| {
                    if data.len() != 0 {
                        Ok((reader, data))
                    } else {
                        Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
                    }
                });

                // Let's not assume valid UTF-8 input. Convert the raw bytes read into a <String>
                let line = line.map(|(reader, data)| unsafe {
                    let ptr = data.as_ptr();
                    let len = data.len();
                    let capacity = data.capacity();

                    mem::forget(data);

                    let s = String::from_raw_parts(ptr as *mut _, len, capacity);

                    (reader, s)
                });

                // Clone `clients_reader` to prepare for moving into closure
                let clients = readers_clients.clone();

                // Send a <Message> to each client on the server except the current one
                line.map(move |(reader, text)| {
                    let mut filtered_clients = clients.lock().unwrap();

                    // Create a <Message> with current client data and serialize it
                    let message = Message {
                        kind: MessageKind::Text,
                        text: text.to_string(),
                        sender: addr,
                        timestamp: SystemTime::now(),
                    };
                    let encoded = serialize(&message).unwrap();

                    // Send `encoded` to all connected clients
                    let iter = filtered_clients
                        .iter_mut()
                        .filter(|&(&k, _)| k != addr)
                        .map(|(_, v)| v);
                    for client in iter {
                        client.unbounded_send(encoded.clone()).unwrap();
                    }

                    reader
                })
            });

            // Clone `clients` to prepare for moving into closure
            let writers_clients = clients.clone();

            // Whenever data is received on the Receiver, write it to the `client_writer`
            let client_writer = rx.fold(writer, move |writer, encoded| {
                // Deserialize the encoded mesage
                let decoded: Message = deserialize(&encoded).unwrap();

                // Create and send acknowledgement if message is <MessageKind::Text>
                if let MessageKind::Text = decoded.kind {
                    let clients = writers_clients.lock().unwrap();

                    let response = Message {
                        kind: MessageKind::Response,
                        text: "".to_string(),
                        sender: addr,
                        timestamp: decoded.timestamp,
                    };
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
                let result = io::write_all(writer, message);
                let result = result.map(|(writer, _)| writer);
                result.map_err(|_| ())
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
