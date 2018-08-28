extern crate bytes;
extern crate futures;
extern crate tokio;

use bytes::{BufMut, BytesMut};
use futures::sync::mpsc;
use tokio::codec::{Decoder, Encoder};
use tokio::net::TcpStream;
use tokio::prelude::*;

use std::env;
use std::io;
use std::net::SocketAddr;
use std::thread;

/// This is a struct used for client I/O. Because the I/O is executed on the event loop, it must be
/// able to be `framed`. By using the `Codec` traits to handle encoding and decoding of message
/// frames, `Bytes` can provide a `Stream` and `Sink` interface for reading and writing this I/O
/// object.
struct Bytes;

impl Decoder for Bytes {
    type Item = BytesMut;
    type Error = io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> io::Result<Option<BytesMut>> {
        if buf.len() > 0 {
            let len = buf.len();
            Ok(Some(buf.split_to(len)))
        } else {
            Ok(None)
        }
    }
}

impl Encoder for Bytes {
    type Item = Vec<u8>;
    type Error = io::Error;

    fn encode(&mut self, data: Vec<u8>, buf: &mut BytesMut) -> io::Result<()> {
        buf.put(&data[..]);
        Ok(())
    }
}

fn connect(
    addr: &SocketAddr,
    stdin: Box<Stream<Item = Vec<u8>, Error = io::Error> + Send>,
) -> Box<Stream<Item = BytesMut, Error = io::Error> + Send> {
    let connection = TcpStream::connect(addr);

    // After establishing a connection, forward all data received on `stdin to` `sink`, and write
    // all data received on `stream` to `stdout`
    Box::new(
        connection
            .map(move |stream| {
                let (sink, stream) = Bytes.framed(stream).split();

                tokio::spawn(stdin.forward(sink).then(|result| {
                    if let Err(e) = result {
                        panic!("failed to write to socket: {}", e)
                    }
                    Ok(())
                }));

                stream
            })
            .flatten_stream(),
    )
}

fn read_stdin(mut tx: mpsc::Sender<Vec<u8>>) {
    let mut stdin = io::stdin();
    loop {
        let mut buf = vec![0; 1024];
        let n = match stdin.read(&mut buf) {
            Err(_) | Ok(0) => break,
            Ok(n) => n,
        };
        buf.truncate(n);
        tx = match tx.send(buf).wait() {
            Ok(tx) => tx,
            Err(_) => break,
        };
    }
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

    // This is Tokio's way of a handle to stdin running on the event loop
    //
    // Read data with blocking I/O (from `read_stdin`) and send it to the event loop over the
    // standard futures channel
    let (tx, rx) = mpsc::channel(0);
    thread::spawn(move || read_stdin(tx));
    let rx = rx.map_err(|_| panic!());

    // Connect to stream of bytes
    let stdout = connect(&addr, Box::new(rx));

    // With the stream of bytes, execute the write to stdout on the event loop
    let mut out = io::stdout();

    tokio::run({
        stdout
            .for_each(move |chunk| out.write_all(&chunk))
            .map_err(|e| println!("error reading stdout; error = {:?}", e))
    })
}
