# chat

A fully asynchronous chat server written on the [Tokio](https://tokio.rs) runtime.

## Overview

chat provides the ability to send `Message` structs from client to client. The current purpose of
this is to allow receiving clients to acknowledge messages from the sending client.

When a client sends a message, each receiving client responds back with the *original* timestamp
of the `Message`. This allows a sending client to then calculate the roundtrip time of messages to
*each* client on the server.

A `Message` struct is sent from thread to thread, so it must be encoded in some way. chat uses
[bincode](https://crates.io/crates/bincode) for this reason. It provides a binary encoding scheme
that is used to `Message` structs between threads.

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
$ hello world!
$ Roundtrip time to 127.0.0.1:56468 was { 0 secs 277000 ns}
$ Roundtrip time to 127.0.0.1:56469 was { 0 secs 357000 ns}
```

Receive messages:
```
$ 127.0.0.1:56513: hello world!
```
