extern crate bincode;
extern crate bytes;
extern crate futures;
#[macro_use]
extern crate serde_derive;
extern crate tokio;

use bincode::{deserialize, serialize};
use bytes::{Bytes, BytesMut};
use futures::future::Future;
use futures::sync::mpsc;
use tokio::codec::{Framed, LengthDelimitedCodec};
use tokio::net::TcpStream;
use tokio::prelude::*;

use std::collections::HashMap;
use std::io;
use std::mem;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

type Tx = mpsc::UnboundedSender<Bytes>;
type Rx = mpsc::UnboundedReceiver<Bytes>;

/// `Client`s store the shared state of connections so that they can send data
/// to the send handles of other clients.
pub struct Shared {
    clients: HashMap<SocketAddr, Tx>,
}

impl Shared {
    pub fn new() -> Self {
        Shared {
            clients: HashMap::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
enum MessageKind {
    Acknowledge,
    Dispatch,
}

#[derive(Deserialize, Serialize)]
struct Message {
    addr: SocketAddr,
    kind: MessageKind,
    name: String,
    text: String,
    timestamp: SystemTime,
}

impl Message {
    fn new(
        addr: SocketAddr,
        kind: MessageKind,
        name: String,
        text: String,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            addr,
            kind,
            name,
            text,
            timestamp,
        }
    }
}

/// The state for each connected client.
pub struct Client {
    /// Socket address of the Client.
    addr: SocketAddr,

    /// The socket wrapped with `LengthDelimitedCodec` codec.
    frames: Framed<TcpStream, LengthDelimitedCodec>,

    /// Name of the Client.
    name: BytesMut,

    /// Shared state of connected clients.
    state: Arc<Mutex<Shared>>,

    /// Receive half of the Client channel.
    recv: Rx,
}

impl Future for Client {
    type Item = ();
    type Error = io::Error;

    fn poll(&mut self) -> Poll<(), Self::Error> {
        const LINES_PER_TICK: usize = 10;

        for i in 0..LINES_PER_TICK {
            match self.recv.poll().unwrap() {
                Async::Ready(Some(v)) => {
                    // let dispatched: Message = deserialize(v.as_ref()).unwrap();
                    // let message = format!("{} - {}", dispatched.name, dispatched.text);

                    println!("Received frame ({:?}) : ({:?})", v.len(), v);

                    self.frames.start_send(v)?;

                    if i + 1 == LINES_PER_TICK {
                        task::current().notify();
                    }
                }

                _ => break,
            }
        }

        self.frames.poll_complete()?;

        while let Async::Ready(frame) = self.frames.poll()? {
            println!("Received line ({:?}) : {:?}", self.addr, frame);

            if let Some(message) = frame {
                // let dispatch = self.make_dispatch(message, self.name.clone());
                let message = message.freeze();

                for (addr, tx) in &self.state.lock().unwrap().clients {
                    if *addr != self.addr {
                        tx.unbounded_send(message.clone()).unwrap();
                    }
                }
            } else {
                return Ok(Async::Ready(()));
            }
        }

        Ok(Async::NotReady)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.state.lock().unwrap().clients.remove(&self.addr);
    }
}

impl Client {
    /// Create a new Client.
    pub fn new(
        addr: SocketAddr,
        frames: Framed<TcpStream, LengthDelimitedCodec>,
        name: BytesMut,
        state: Arc<Mutex<Shared>>,
    ) -> Self {
        // Create a channel for this client
        let (send, recv) = mpsc::unbounded();

        // Add an entry for this `Client` in the shared state map
        state.lock().unwrap().clients.insert(addr, send);

        Client {
            addr,
            frames,
            name,
            state,
            recv,
        }
    }

    fn make_dispatch(&self, message: BytesMut, name: BytesMut) -> Vec<u8> {
        let text = unsafe {
            let ptr = message.as_ptr();
            let len = message.len();
            let cap = message.capacity();

            mem::forget(message);
            String::from_raw_parts(ptr as *mut _, len, cap)
        };
        let name = unsafe {
            let ptr = name.as_ptr();
            let len = name.len();
            let cap = name.capacity();

            mem::forget(name);
            String::from_raw_parts(ptr as *mut _, len, cap)
        };
        let message = Message::new(
            self.addr,
            MessageKind::Dispatch,
            name,
            text,
            SystemTime::now(),
        );
        serialize(&message).unwrap()
    }
}

/// Convenience function for reading from stdin.
pub fn read_stdin(mut stdin_send: mpsc::Sender<Bytes>) {
    let mut stdin = io::stdin();
    loop {
        let mut buf = vec![0; 1024];
        let n = match stdin.read(&mut buf) {
            Err(_) | Ok(0) => break,
            Ok(n) => n,
        };
        buf.truncate(n);
        stdin_send = match stdin_send.send(Bytes::from(buf)).wait() {
            Ok(tx) => tx,
            Err(_) => break,
        };
    }
}

/// Convenience function for establishing a connection and start forwarding
/// data.
pub fn connect(
    addr: &SocketAddr,
    stdin: Box<Stream<Item = Bytes, Error = io::Error> + Send>,
) -> Box<Stream<Item = BytesMut, Error = io::Error> + Send> {
    let connection = TcpStream::connect(addr);

    Box::new(
        connection
            .map(move |socket| {
                // Take all data that we receive on `stdin` and forward that
                // to the `send` half of the stream.
                //
                // Take the `recv` stream handle and pass that back for
                // writing to stdout.
                let (send, recv) = Framed::new(socket, LengthDelimitedCodec::new()).split();

                tokio::spawn(stdin.forward(send).then(|result| {
                    if let Err(e) = result {
                        panic!("failed to write to socket: {}", e)
                    }
                    Ok(())
                }));

                recv
            }).flatten_stream(),
    )
}
