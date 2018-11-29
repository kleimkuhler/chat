# chat

A fully asynchronous chat server written on the [Tokio](https://tokio.rs)
runtime.

## Overview

chat provides the ability to send `Message` structs from client to client. The
current purpose of this is to allow receiving clients to acknowledge messages
from the sending client.

When a client sends a message, each receiving client responds back with the
*original* timestamp of the `Message`. This allows a sending client to then
calculate the roundtrip time of messages to *each* client on the server.

A `Message` struct is sent from thread to thread, so it must be encoded in
some way. chat uses [bincode](https://crates.io/crates/bincode) for this
reason. It provides a binary encoding scheme that is used to `Message` structs
between threads.

Currently, the protocol that this chat uses assumes nothing about the contents
of the message. All streams are wrapped with a `LengthDelimitedCodec` which
allows the protcol to frame messages. The use of frames mean there is no set
delimiter by which to parse messages.

## Example

### Start server and connect

chat starts in two modes: `server` and `client`.

Start the server on `127.0.0.1:12345`:
```
$ cargo run -p server 127.0.0.1:12345
```

Connect any number of clients:
```
$ cargo run -p client 127.0.0.1:12345
```

### Begin chatting!

Send messages (and notice the acknowledgements from each connected client!):
```
$ Please enter your name:
$ Foo
$ hello world!
$ <--> Roundtrip time to 127.0.0.1:52943 was { 0 secs 540000 ns}
$ <--> Roundtrip time to 127.0.0.1:52944 was { 0 secs 593000 ns}
```

Receive messages:
```
$ Foo: hello world!
```

Server output:
```
$ Server running on 127.0.0.1:12345
$ b"Baz" joined the chat
$ b"Bar" joined the chat
$ b"Foo" joined the chat
$ Received frame (V4(127.0.0.1:52942)) : Some(b"hello world!\n")
```
