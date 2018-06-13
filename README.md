HeadsUp
=======

A very basic peer-to-peer or multi-user chat application.


### Installation and Usage

Ensure that [Rust](https://www.rust-lang.org/en-US/) is installed. Clone this repository, change directories, then type `cargo run`.

You can specify the IP and port to listen on by using the `--server` command line switch, e.g.:

```
cargo run -- --server localhost:5000
```

Use the `--client` switch to specify a server url to connect to upon startup:

```
cargo run -- --server localhost:5000 --client cogciprocate.com:3030
```

After opening application, type `/connect {url}` to connect to a server. Type
`/help` at any time for a complete list of commands.


#### Problems?

This is an experimental for-fun project but please feel free to report any
problems or ask questions on the
[Issues](https://github.com/c0gent/headsup/issues) page.


#### `ws-rs` Library Problems:

* There are some errors that are not propagated back out to the `on_error`
  handler and that are only detectable when logging is enabled. (Relevant
  issue: https://github.com/housleyjk/ws-rs/issues/155).
  * UPDATE: Further investigation has lead to the source of the unpropagated
    error. See my comment in the above issue thread.
* Some errors cause a panic rather than propagating an error. One example
  is that a client can be running but is not yet connected. When the user
  tries to close this connection, it causes a panic rather than
  propagating an error even though this is a very recoverable error.
  Further investigation required before filing an issue.
  * UPDATE: Solved: Using [`ws::Builder`](https://ws-rs.org/api_docs/ws/struct.Builder.html)
    and disabling the [`panic_on_internal`](https://ws-rs.org/api_docs/ws/struct.Settings.html#structfield.panic_on_internal) setting alleviates
    this.
* Conclusion: The `ws-rs` library may not quite be robust enough to be used in
  a production environment, but is probably close.


##### License

Licensed under the MIT license ([LICENSE](LICENSE) or http://opensource.org/licenses/MIT).