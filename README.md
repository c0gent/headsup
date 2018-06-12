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


##### License

Licensed under the MIT license ([LICENSE](LICENSE) or http://opensource.org/licenses/MIT).