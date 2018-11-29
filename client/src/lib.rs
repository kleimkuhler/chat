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
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

type Tx = mpsc::UnboundedSender<Bytes>;
type Rx = mpsc::UnboundedReceiver<Bytes>;

/// `Client`s store the shared state of connections so that they can send data
/// to the send handles of other clients.
#[derive(Default)]
pub struct Shared {
    clients: HashMap<SocketAddr, Tx>,
}

// impl Shared {
//     pub fn new() -> Self {
//         Shared {
//             clients: HashMap::new(),
//         }
//     }
// }

/// `MessageKind` is used by the receiver to decide whether to acknowledge
/// a sender
#[derive(Deserialize, Serialize)]
enum MessageKind {
    Acknowledge,
    Dispatch,
}

/// `Message` is used to package the fields required for constructing an
/// acknowledgement that a message has been received.
#[derive(Deserialize, Serialize)]
struct Message {
    addr: SocketAddr,
    kind: MessageKind,
    name: BytesMut,
    text: BytesMut,
    timestamp: SystemTime,
}

impl Message {
    fn new(
        addr: SocketAddr,
        kind: MessageKind,
        name: BytesMut,
        text: BytesMut,
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

        // If the limit is hit, the current task is notified, informing the
        // executor to schedule the task again asap.
        for i in 0..LINES_PER_TICK {
            match self.recv.poll().unwrap() {
                Async::Ready(Some(message)) => {
                    let message: Message = deserialize(message.as_ref()).unwrap();
                    let output = match message.kind {
                        MessageKind::Acknowledge => self.handle_acknowledge(message),
                        MessageKind::Dispatch => self.handle_dispatch(message),
                    };
                    self.frames.start_send(output)?;

                    if i + 1 == LINES_PER_TICK {
                        task::current().notify();
                    }
                }

                _ => break,
            }
        }

        // Flush all output from the client sink
        self.frames.poll_complete()?;

        // While there are values in the client stream, pull them out and
        // dispatch to all connected clients
        while let Async::Ready(frame) = self.frames.poll()? {
            println!("Received frame ({:?}) : {:?}", self.addr, frame);

            if let Some(message) = frame {
                let dispatch = self.handle_input(message);
                for (addr, tx) in &self.state.lock().unwrap().clients {
                    if *addr != self.addr {
                        tx.unbounded_send(dispatch.clone()).unwrap();
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

    /// Create an output message for the original sender when a
    /// `MessageKind::Acknowledge` is received.
    fn handle_acknowledge(&self, message: Message) -> Bytes {
        let now = SystemTime::now();
        let roundtrip = now.duration_since(message.timestamp).unwrap();
        let acknowledgement = format!(
            "<--> Roundtrip time to {} was {{ {} secs {} ns}}\n",
            message.addr,
            roundtrip.as_secs(),
            roundtrip.subsec_nanos()
        );

        Bytes::from(acknowledgement)
    }

    /// Create an acknowledgement message for the sender and send it back.
    /// Then create an output message for the receiving client.
    fn handle_dispatch(&self, message: Message) -> Bytes {
        let acknowledge = {
            let message = Message::new(
                self.addr,
                MessageKind::Acknowledge,
                self.name.clone(),
                BytesMut::new(),
                message.timestamp,
            );
            serialize(&message).unwrap()
        };
        let clients = &self.state.lock().unwrap().clients;
        let sender = clients.get(&message.addr).unwrap();
        sender.unbounded_send(Bytes::from(acknowledge)).unwrap();

        // Construct the message to be output to all connected clients
        let mut dispatch = message.name;
        dispatch.extend_from_slice(b": ");
        dispatch.extend(message.text);
        dispatch.extend(b"\n");

        dispatch.freeze()
    }

    /// Create a dispatch message to send to all other clients.
    fn handle_input(&self, input: BytesMut) -> Bytes {
        let stripped = strip_newline(input);
        let message = {
            let dispatch = Message::new(
                self.addr,
                MessageKind::Dispatch,
                self.name.clone(),
                stripped.clone(),
                SystemTime::now(),
            );
            serialize(&dispatch).unwrap()
        };

        Bytes::from(message)
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

/// Convenience function for stripping newline ('\n') characters from the end
/// of client input. This does not assume there is one to strip, but leads to
/// cleaner server and client output.
pub fn strip_newline(mut input: BytesMut) -> BytesMut {
    let len = input.len();
    match &input[len - 1] {
        &b'\n' => {
            input.truncate(len - 1);
        }

        _ => (),
    };
    input
}
